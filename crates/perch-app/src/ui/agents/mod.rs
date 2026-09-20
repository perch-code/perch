//! App-wide agent activity.
//!
//! The inspector is an overlay rather than another workspace column. Opening
//! it therefore leaves the editor, its selection, the conversation scroll and
//! any composer exactly where the reader put them.

pub mod model;

mod detail;
mod list;

use std::rc::Rc;
use std::time::Duration;

use floem::event::{DragConfig, EventPropagation};
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Effect, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position};
use floem::text::FontWeight;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{ItemTab, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::titlebar::title_bar;
use crate::ui::widgets::{clickable, sidebar_glyph};

use model::{AgentRunStatus, AgentRunSummary};

const FIRST_HISTORY_PAGE: usize = 50;

/// The persistent entry in the status bar. Its count is deliberately global:
/// filters in the inspector never make running work disappear from chrome.
pub fn control(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let label = move || {
        let (running, queued) = state.active_agent_counts();
        match (running, queued) {
            (0, 0) => "Agents".to_string(),
            (running, 0) => format!("Agents · {running} running"),
            (0, queued) => format!("Agents · {queued} queued"),
            (running, queued) => format!("Agents · {running} running · {queued} queued"),
        }
    };
    let inner = Stack::horizontal((
        sidebar_glyph(SidebarIcon::Agents, theme::FG_MUTED)
            .style(|s| s.size(13.0, 13.0).margin_right(6.0).pointer_events_none()),
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_MUTED)
                .selectable(false)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
                .pointer_events_none()
        }),
        dyn_container(
            move || state.active_agent_counts().0 > 0,
            |active| {
                if active {
                    Empty::new()
                        .style(|s| {
                            s.size(6.0, 6.0)
                                .border_radius(3.0)
                                .background(theme::ACCENT)
                                .margin_left(7.0)
                        })
                        .into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(|s| s.pointer_events_none()),
    ))
    .style(|s| s.items_center().pointer_events_none());

    let button = Container::new(inner);
    let id = button.view_id();
    state.agents_control.set(Some(id));
    let click_backend = backend.clone();
    button
        .on_event_stop(el::Click, move |_, _| {
            toggle(state, &click_backend, Some(id))
        })
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                toggle(state, &backend, Some(id));
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| {
            let base = clickable(s)
                .height(24.0)
                .items_center()
                .padding_horiz(8.0)
                .border_radius(theme::RADIUS)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
                .hover(|s| s.background(theme::CANVAS_HOVER));
            if state.agents_open.get() {
                base.background(theme::BADGE_BG)
            } else {
                base
            }
        })
        .debug_name("agents-control")
}

pub fn toggle(state: AppState, backend: &Backend, opener: Option<floem::ViewId>) {
    let open = !state.agents_open.get_untracked();
    state.agents_open.set(open);
    backend.set_meta("agents.open", if open { "true" } else { "false" });
    if open {
        if let Some(opener) = opener {
            state.agents_opener.set(Some(opener));
        }
    } else {
        state.selected_agent_run.set(None);
        state.pending_agent_row_focus.set(None);
        restore_opener(state);
    }
}

pub fn close(state: AppState, backend: &Backend, restore_focus: bool) {
    state.agents_open.set(false);
    state.selected_agent_run.set(None);
    state.pending_agent_row_focus.set(None);
    backend.set_meta("agents.open", "false");
    if restore_focus {
        restore_opener(state);
    } else {
        state.agents_opener.set(None);
    }
}

fn restore_opener(state: AppState) {
    let opener = state.agents_opener.get_untracked();
    state.agents_opener.set(None);
    if let Some(opener) = opener {
        opener.request_focus();
    }
}

/// The overlay itself. It stays a sibling of the workspace instead of being
/// keyed into it, which is what preserves all item-local view state.
pub fn panel(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let load_backend = backend.clone();
    Effect::new(move |previous: Option<(bool, bool)>| {
        let open = state.agents_open.get();
        let active = state
            .agent_runs
            .with(|runs| runs.iter().any(|run| run.status.is_active()));
        if open && !previous.is_some_and(|(was_open, _)| was_open) {
            request_agent_runs(state, &load_backend, FIRST_HISTORY_PAGE);
        }
        if open && active && !state.agents_clock_armed.get_untracked() {
            arm_clock(state);
        }
        (open, active)
    });
    dyn_container(
        move || state.agents_open.get(),
        move |open| {
            if !open {
                return Empty::new().into_any();
            }
            surface(state, review, backend.clone()).into_any()
        },
    )
    .style(|s| {
        s.position(Position::Absolute)
            .inset_top(0.0)
            .inset_right(0.0)
            .inset_bottom(theme::STATUS_HEIGHT)
            .z_index(60)
    })
    .debug_name("agents-panel-slot")
}

pub(super) fn request_agent_runs(state: AppState, backend: &Backend, history_limit: usize) {
    let token = state.begin_agent_history_load();
    backend.load_agent_runs(history_limit);
    floem::action::exec_after(Duration::from_secs(5), move |_| {
        state.time_out_agent_history_load(token);
    });
}

fn surface(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let header_backend = backend.clone();
    let body_backend = backend.clone();
    Container::new(
        Stack::horizontal((
            resize_handle(state, backend.clone()),
            Stack::vertical((
                panel_header(state, header_backend),
                dyn_container(
                    move || state.selected_agent_run.get(),
                    move |selected| match selected.and_then(|id| state.agent_run(&id)) {
                        Some(run) => {
                            detail::view(state, review, body_backend.clone(), run).into_any()
                        }
                        None => list::view(state, review, body_backend.clone()).into_any(),
                    },
                )
                .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
            ))
            .style(|s| s.size_full().flex_col().min_width(0.0)),
        ))
        .style(|s| s.size_full()),
    )
    .style(move |s| {
        s.width(state.agents_width.get())
            .max_width_full()
            .height_full()
            .background(theme::CANVAS)
            .border_left(1.0)
            .border_color(theme::BORDER)
            .box_shadow_blur(18.0)
            .box_shadow_h_offset(-5.0)
            .box_shadow_color(theme::SHADOW)
            .position(Position::Relative)
    })
    .debug_name("agents-panel")
}

fn panel_header(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let back = dyn_container(
        move || state.selected_agent_run.with(Option::is_some),
        move |selected| {
            if !selected {
                return Empty::new().into_any();
            }
            let back = icon_action(
                SidebarIcon::ChevronLeft,
                "agents-detail-back Back to agents",
                move || back_to_list(state),
            );
            let id = back.view_id();
            // The selected row has just left the tree. Put the keyboard on a
            // stable detail control after Floem installs this replacement.
            let asked = std::rc::Rc::new(std::cell::Cell::new(false));
            back.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
                if !asked.replace(true) {
                    id.request_focus();
                }
            })
            .into_any()
        },
    );
    let title = Label::derived(move || {
        if state.selected_agent_run.with(Option::is_some) {
            "Run details".to_string()
        } else {
            "Agents".to_string()
        }
    })
    .style(|s| {
        s.font_size(theme::TEXT_TITLE)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(theme::FG)
            .selectable(false)
            .pointer_events_none()
    });
    let count = Label::derived(move || {
        let (running, queued) = state.active_agent_counts();
        let total = running + queued;
        if total == 0 {
            String::new()
        } else {
            total.to_string()
        }
    })
    .style(move |s| {
        s.font_size(theme::TEXT_SECTION)
            .font_weight(FontWeight::MEDIUM)
            .color(theme::FG_MUTED)
            .height(18.0)
            .padding_horiz(6.0)
            .border_radius(9.0)
            .background(theme::BADGE_BG)
            .items_center()
            .justify_center()
            .selectable(false)
            .pointer_events_none()
            .apply_if(state.active_agent_counts() == (0, 0), |s| s.hide())
    });
    let close_backend = backend.clone();
    let close_button = icon_action(SidebarIcon::Close, "agents-close Close agents", move || {
        close(state, &close_backend, true)
    });
    let close_id = close_button.view_id();
    let focus_close = state.selected_agent_run.get_untracked().is_none();
    let asked = std::rc::Rc::new(std::cell::Cell::new(false));
    let close_button = close_button.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if focus_close && !asked.replace(true) {
            close_id.request_focus();
        }
    });

    title_bar(
        Stack::horizontal((
            back,
            title,
            count,
            Empty::new().style(|s| s.flex_grow(1.0_f32).pointer_events_none()),
            close_button,
        ))
        .style(|s| {
            s.items_center()
                .width_full()
                .gap(8.0)
                .padding_left(12.0)
                .padding_right(8.0)
        }),
    )
    .style(|s| {
        s.height(theme::AGENTS_HEADER_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .background(theme::CANVAS)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

pub fn back_to_list(state: AppState) {
    let selected = state.selected_agent_run.get_untracked();
    state.pending_agent_row_focus.set(selected);
    state.selected_agent_run.set(None);
}

pub(super) fn icon_action(
    icon: SidebarIcon,
    debug: &'static str,
    action: impl Fn() + 'static,
) -> impl View {
    let action: Rc<dyn Fn()> = Rc::new(action);
    let click = action.clone();
    Container::new(
        sidebar_glyph(icon, theme::FG_MUTED).style(|s| s.size(14.0, 14.0).pointer_events_none()),
    )
    .on_event_stop(el::Click, move |_, _| click())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            action();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| {
        clickable(s)
            .size(28.0, 28.0)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
    .debug_name(debug)
    .into_view()
}

pub(super) fn text_action(
    text: impl Fn() -> String + 'static,
    prominent: bool,
    action: impl Fn() + 'static,
) -> impl IntoView {
    let action: Rc<dyn Fn()> = Rc::new(action);
    let click = action.clone();
    Container::new(Label::derived(text).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(if prominent {
                FontWeight::SEMI_BOLD
            } else {
                FontWeight::MEDIUM
            })
            .color(if prominent {
                theme::ACCENT
            } else {
                theme::FG_MUTED
            })
            .selectable(false)
            .pointer_events_none()
            .margin_bottom(theme::OPTICAL_TEXT_LIFT)
    }))
    .on_event_stop(el::Click, move |_, _| click())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            action();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let base = clickable(s)
            .height(27.0)
            .items_center()
            .padding_horiz(8.0)
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING));
        if prominent {
            base.background(theme::ACCENT_SUBTLE)
                .hover(|s| s.background(theme::BADGE_BG))
        } else {
            base.hover(|s| s.background(theme::CANVAS_HOVER))
        }
    })
}

pub(super) fn status_color(status: AgentRunStatus) -> Color {
    match status {
        AgentRunStatus::Running => theme::ACCENT,
        AgentRunStatus::Completed => theme::OPEN,
        AgentRunStatus::Failed => theme::CLOSED,
        AgentRunStatus::Stopped => theme::FG_SUBTLE,
        AgentRunStatus::Queued
        | AgentRunStatus::Starting
        | AgentRunStatus::Stopping
        | AgentRunStatus::Interrupted => theme::DRAFT_YELLOW,
    }
}

pub(super) fn metadata(run: &AgentRunSummary) -> String {
    let model = crate::review::threads::model_label(&run.model);
    let effort = perch_agent::Effort::parse(&run.effort)
        .map(|effort| effort.label().to_string())
        .unwrap_or_else(|| run.effort.clone());
    format!("{} · {model} · {effort}", run.backend_label())
}

pub(super) fn navigate_to_source(
    app: AppState,
    review: ReviewState,
    backend: &Backend,
    run: &AgentRunSummary,
) {
    let Some(item_id) = run.item_id.clone().filter(|_| run.source_available) else {
        app.notice.set(Some(
            "The source for this run is no longer available".into(),
        ));
        return;
    };
    if app.item(&item_id).is_none() {
        app.notice.set(Some(
            "The source for this run is no longer available".into(),
        ));
        return;
    }
    let same_item = app
        .open_item
        .with_untracked(|open| open.as_deref() == Some(item_id.as_str()))
        && review.is_showing();
    let same_live_item = can_focus_loaded_discussion(
        same_item,
        review.historical_review.with_untracked(Option::is_some),
    );
    close(app, backend, false);
    if let Some(attempt_id) = run.review_attempt_id.clone() {
        let navigation_id = backend.view_review_attempt_owner(&item_id, &attempt_id);
        app.begin_agent_review_navigation(
            review,
            item_id.clone(),
            attempt_id.clone(),
            navigation_id,
        );
        return;
    }
    review.show_tab(ItemTab::Conversation);
    if same_live_item {
        if let Some(thread_id) = &run.thread_id {
            review
                .focus_card
                .set(Some(crate::review::doc::CardId::Thread(thread_id.clone())));
        }
        return;
    }
    let navigation_id = backend.view_agent_owner(&item_id);
    app.begin_agent_navigation(item_id.clone(), run.thread_id.clone(), navigation_id);
    app.begin_review(review, "Reading the discussion");
}

fn can_focus_loaded_discussion(same_item: bool, historical_review: bool) -> bool {
    same_item && !historical_review
}

fn arm_clock(state: AppState) {
    if state.agents_clock_armed.get_untracked() {
        return;
    }
    state.agents_clock_armed.set(true);
    tick_clock(state);
}

fn tick_clock(state: AppState) {
    floem::action::exec_after(Duration::from_secs(1), move |_| {
        let active = state
            .agent_runs
            .with_untracked(|runs| runs.iter().any(|run| run.status.is_active()));
        if state.agents_open.get_untracked() && active {
            state.now.set(jiff::Timestamp::now());
            tick_clock(state);
        } else {
            state.agents_clock_armed.set(false);
        }
    });
}

fn resize_handle(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let start_width = RwSignal::new(theme::AGENTS_WIDTH);
    Empty::new()
        .on_event_stop(el::PointerDown, |cx, event: &PointerButtonEvent| {
            if let Some(pointer) = event
                .pointer
                .pointer_id
                .filter(|pointer| pointer.is_primary_pointer())
            {
                cx.request_pointer_capture(pointer);
            }
        })
        .on_event_stop(el::GainedPointerCapture, |cx, token| {
            cx.start_drag(
                *token,
                DragConfig {
                    threshold: 1.0,
                    animation_duration: Duration::ZERO,
                    easing: Rc::new(floem::easing::Linear),
                    custom_data: None,
                    track_targets: false,
                },
                false,
            );
        })
        .on_event_stop(el::DragStart, move |_, _: &floem::event::DragStartEvent| {
            start_width.set(state.agents_width.get_untracked());
            state.agents_resizing.set(true);
        })
        .on_event_stop(
            el::DragMove,
            move |cx, event: &floem::event::DragMoveEvent| {
                let inverse = cx.world_transform.inverse();
                let start = inverse * event.start_state.logical_point();
                let current = inverse * event.current_state.logical_point();
                let width = (start_width.get_untracked() + start.x - current.x)
                    .clamp(theme::AGENTS_MIN_WIDTH, theme::AGENTS_MAX_WIDTH);
                state.agents_width.set(width);
            },
        )
        .on_event_stop(el::DragEnd, move |_, _: &floem::event::DragEndEvent| {
            state.agents_resizing.set(false);
            backend.set_meta(
                "agents.width",
                &state.agents_width.get_untracked().round().to_string(),
            );
        })
        .on_event_stop(
            el::DragCancel,
            move |_, _: &floem::event::DragCancelEvent| {
                state.agents_resizing.set(false);
            },
        )
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(0.0)
                .inset_bottom(0.0)
                .inset_left(-4.0)
                .width(8.0)
                .z_index(3)
                .hover(|s| s.cursor(CursorStyle::ColResize))
        })
}

/// Keep the resize cursor while capture carries the pointer over the editor.
pub fn resize_cursor_sheet(state: AppState) -> impl IntoView {
    Empty::new().style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            .z_index(101)
            .cursor(CursorStyle::ColResize)
            .apply_if(!state.agents_resizing.get(), |s| s.hide())
    })
}

/// A terminal transition stays lightweight: enough context to identify it,
/// plus a path into the durable detail. A later completion replaces an older
/// one, and AppState's tokenized timer dismisses only the notice it scheduled.
pub fn completion_notice(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || {
            state.current_agent_completion().and_then(|notice| {
                state
                    .agent_run(&notice.run_id)
                    .map(|run| (notice.token, run))
            })
        },
        move |notice| {
            let Some((token, run)) = notice else {
                return Empty::new().into_any();
            };
            let color = status_color(run.status);
            let title = format!(
                "{} {}",
                run.backend_label(),
                run.status.label().to_lowercase()
            );
            let context = {
                let group = run.group_label();
                if group.is_empty() {
                    run.purpose.label().to_string()
                } else {
                    format!("{} · {group}", run.purpose.label())
                }
            };
            let run_id = run.id.clone();
            let view_backend = backend.clone();
            let view = text_action(
                || "View".to_string(),
                true,
                move || {
                    state.dismiss_agent_completion(token);
                    state
                        .agents_opener
                        .set(state.agents_control.get_untracked());
                    state.agents_open.set(true);
                    state.agents_tab.set(model::AgentsTab::History);
                    state.selected_agent_run.set(Some(run_id.clone()));
                    state.set_agent_run_events_loading(&run_id);
                    view_backend.set_meta("agents.open", "true");
                    view_backend.load_agent_run_detail(&run_id);
                },
            );
            let dismiss = icon_action(SidebarIcon::Close, "Dismiss agent completion", move || {
                state.dismiss_agent_completion(token)
            });
            Container::new(
                Stack::horizontal((
                    Empty::new().style(move |s| {
                        s.size(9.0, 9.0)
                            .border_radius(5.0)
                            .background(color)
                            .flex_shrink(0.0_f32)
                    }),
                    Stack::vertical((
                        Label::derived(move || title.clone()).style(|s| {
                            s.font_size(theme::TEXT_BODY)
                                .font_weight(FontWeight::SEMI_BOLD)
                                .color(theme::FG)
                                .selectable(false)
                        }),
                        Label::derived(move || context.clone()).style(|s| {
                            s.font_size(theme::TEXT_BADGE)
                                .color(theme::FG_SUBTLE)
                                .text_ellipsis()
                                .min_width(0.0)
                                .selectable(false)
                        }),
                    ))
                    .style(|s| s.flex_col().gap(3.0).min_width(0.0).flex_grow(1.0_f32)),
                    view,
                    dismiss,
                ))
                .style(|s| s.items_center().width_full().gap(9.0)),
            )
            .style(|s| {
                s.width(328.0)
                    .max_width_full()
                    .padding_left(13.0)
                    .padding_right(7.0)
                    .padding_vert(9.0)
                    .background(theme::CANVAS)
                    .border(1.0)
                    .border_color(theme::BORDER)
                    .border_radius(8.0)
                    .box_shadow_blur(18.0)
                    .box_shadow_v_offset(5.0)
                    .box_shadow_color(theme::SHADOW)
            })
            .into_any()
        },
    )
    .style(|s| {
        s.position(Position::Absolute)
            .inset_right(12.0)
            .inset_bottom(theme::STATUS_HEIGHT + 12.0)
            .z_index(80)
    })
    .debug_name("agent-completion-notice")
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        named(harness.root_id(), name).is_some_and(|id| harness.get_layout_rect(id).height() > 0.0)
    }

    fn key(code: Code, key: Key, modifiers: Modifiers) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    fn running_record() -> perch_github::RunRecord {
        perch_github::RunRecord {
            id: "stream".into(),
            owner: perch_github::RunOwner::Unavailable {
                legacy_thread_id: None,
            },
            backend: "claude-code".into(),
            requested_model: "sonnet".into(),
            model: "sonnet".into(),
            requested_effort: "high".into(),
            effort: "high".into(),
            original_request: "Explain the scheduler".into(),
            status: perch_github::RunStatus::Running,
            queued_at: "2026-09-19T10:00:00Z".parse().unwrap(),
            started_at: Some("2026-09-19T10:00:01Z".parse().unwrap()),
            latest_activity: Some("Reading the queue".into()),
            last_event_sequence: 1,
            ..perch_github::RunRecord::default()
        }
    }

    #[test]
    fn the_keyboard_overlay_filters_streams_and_preserves_the_workspace() {
        let root = TestRoot::new();
        let app = AppState::new();
        let review = ReviewState::new();
        review.composer_draft.set("half a thought".into());
        app.merge_agent_runs([perch_github::RunSummary::from(&running_record())], false);
        let backend = Rc::new(Backend::detached());
        let view = crate::ui::app_view(app, review, backend);
        let mut harness = HeadlessHarness::new_with_size(root, view, 1200.0, 760.0);
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "agent-run:stream"));
        let control = named(harness.root_id(), "agents-control").unwrap();
        control.request_focus();
        harness.rebuild();
        assert!(harness.is_focused(control));

        harness.dispatch_event(key(
            Code::KeyA,
            Key::Character("a".into()),
            crate::ui::OS_MOD | Modifiers::SHIFT,
        ));
        harness.rebuild();
        harness.paint();
        assert!(app.agents_open.get());
        assert!(shown(&harness, "agent-run:stream"));
        let close = named(harness.root_id(), "agents-close").unwrap();
        assert!(harness.is_focused(close));

        app.agents_backend_filter.set(model::BackendFilter::Codex);
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "agent-run:stream"));
        app.agents_backend_filter.set(model::BackendFilter::All);
        harness.rebuild();
        harness.paint();

        let row = named(harness.root_id(), "agent-run:stream").unwrap();
        row.request_focus();
        harness.rebuild();
        harness.dispatch_event(key(
            Code::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::empty(),
        ));
        let mut update = running_record();
        update.final_result = Some("The first durable paragraph.".into());
        update.latest_activity = Some("Responding".into());
        update.last_event_sequence = 2;
        app.update_agent_run(&update);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "agent-run-detail"));
        let back = named(harness.root_id(), "agents-detail-back").unwrap();
        assert!(harness.is_focused(back));

        harness.dispatch_event(key(
            Code::Escape,
            Key::Named(NamedKey::Escape),
            Modifiers::empty(),
        ));
        harness.rebuild();
        harness.paint();
        assert!(app.agents_open.get());
        assert!(app.selected_agent_run.get().is_none());
        assert!(shown(&harness, "agent-run:stream"));
        let rebuilt_row = named(harness.root_id(), "agent-run:stream").unwrap();
        assert!(harness.is_focused(rebuilt_row));

        harness.dispatch_event(key(
            Code::Escape,
            Key::Named(NamedKey::Escape),
            Modifiers::empty(),
        ));
        harness.rebuild();
        harness.paint();
        assert!(!app.agents_open.get());
        assert_eq!(review.composer_draft.get(), "half a thought");
        assert!(harness.is_focused(control));
    }

    #[test]
    fn an_open_idle_panel_rearms_its_clock_when_work_arrives() {
        let root = TestRoot::new();
        let app = AppState::new();
        let review = ReviewState::new();
        app.agents_open.set(true);
        let backend = Rc::new(Backend::detached());
        let view = crate::ui::app_view(app, review, backend);
        let mut harness = HeadlessHarness::new_with_size(root, view, 900.0, 700.0);
        harness.rebuild();
        assert!(!app.agents_clock_armed.get());

        app.merge_agent_runs([perch_github::RunSummary::from(&running_record())], false);
        harness.rebuild();
        assert!(app.agents_clock_armed.get());
    }

    #[test]
    fn panel_contents_fill_the_allocated_width() {
        let root = TestRoot::new();
        let app = AppState::new();
        let review = ReviewState::new();
        app.agents_open.set(true);
        app.agents_tab.set(model::AgentsTab::History);
        let backend = Rc::new(Backend::detached());
        let view = crate::ui::app_view(app, review, backend);
        let mut harness = HeadlessHarness::new_with_size(root, view, 1200.0, 760.0);
        harness.rebuild();
        harness.paint();

        let panel = named(harness.root_id(), "agents-panel")
            .expect("the open panel exists")
            .get_visual_rect_no_clip();
        let list = named(harness.root_id(), "agents-list")
            .expect("the history list exists")
            .get_visual_rect_no_clip();
        assert_eq!(panel.width(), theme::AGENTS_WIDTH);
        assert!(
            (panel.x1 - list.x1).abs() < 0.01,
            "the panel content must reach the panel's right edge: panel={panel:?}, list={list:?}"
        );
    }

    #[test]
    fn ordinary_view_reloads_a_same_item_historical_snapshot() {
        assert!(can_focus_loaded_discussion(true, false));
        assert!(!can_focus_loaded_discussion(true, true));
    }
}
