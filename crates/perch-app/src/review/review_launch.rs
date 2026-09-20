//! Native branch-review launch, progress, and retained review history.
//!
//! Findings themselves remain ordinary `CommentThread`s. This module owns
//! only the group-level controls and summaries around those threads.

use std::cell::Cell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{Effect, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position};
use floem::text::FontWeight;
use floem::views::{dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{
    ReviewGroupSummary, ReviewLaunchChoice, ReviewLaunchDraft, ReviewLaunchRequest,
    ReviewPreferenceChoice, ReviewState,
};
use crate::state::AppState;
use crate::theme;
use crate::ui::agents::model::{AgentsTab, BackendFilter, WorktreeFilter};
use crate::ui::widgets::{clickable, sidebar_glyph};

/// Header entry point. An active review opens Agents; otherwise it opens the
/// launch surface without changing the review's current tab or file.
pub fn control(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // History and an in-flight group's progress belong in the worktree view,
    // not behind the launcher's first click. Reload when the open item changes;
    // opening the popover below still performs an explicit refresh.
    let dashboard_backend = backend.clone();
    Effect::new(move |previous: Option<Option<String>>| {
        let item_id = current_item_id(state);
        if previous.as_ref() != Some(&item_id) {
            if let Some(item_id) = item_id.as_deref() {
                app.begin_review_dashboard_load(item_id);
                dashboard_backend.load_review_dashboard(item_id);
            }
        }
        item_id
    });

    let sync_app = app;
    Effect::new(move |_| {
        let open = state.review_launcher_open.get();
        let _capabilities = sync_app.review_capabilities.get();
        let _preferences = sync_app.review_preferences.get();
        let _agent = sync_app.agent.get();
        let item_id = state
            .item
            .with(|item| item.as_ref().map(|item| item.id.clone()));
        let _base_refs = item_id
            .as_deref()
            .map(|item_id| sync_app.review_base_refs_for(item_id));
        if open {
            if let Some(item_id) = item_id {
                state
                    .review_launcher_draft
                    .set(build_draft(sync_app, state, &item_id));
            }
        }
    });

    let label = move || {
        active_group(app, state)
            .map(|group| progress_label(&group))
            .unwrap_or_else(|| "Review".to_string())
    };
    let inner = Stack::horizontal((
        sidebar_glyph(SidebarIcon::Agents, theme::FG_MUTED)
            .style(|s| s.size(13.0, 13.0).margin_right(6.0).pointer_events_none()),
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(FontWeight::MEDIUM)
                .color(theme::FG)
                .selectable(false)
                .pointer_events_none()
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE)
            .style(|s| s.size(9.0, 9.0).margin_left(6.0).pointer_events_none()),
    ))
    .style(|s| s.items_center().pointer_events_none());
    let button = Container::new(inner);
    let button_id = button.view_id();
    let click_backend = backend.clone();
    let click: Rc<dyn Fn()> = Rc::new(move || {
        if active_group(app, state).is_some() {
            open_agents(app, state, &click_backend, button_id);
        } else if state.review_launcher_open.get_untracked() {
            close_launcher(state, true);
        } else {
            open_launcher(app, state, &click_backend, Some(button_id));
        }
    });
    let keyboard = click.clone();
    let button = button
        .on_event_stop(el::Click, move |_, _| click())
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                keyboard();
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| {
            let base = clickable(s)
                .height(26.0)
                .padding_horiz(9.0)
                .items_center()
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .background(theme::CANVAS)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
                .hover(|s| s.background(theme::CANVAS_HOVER));
            if state.review_launcher_open.get() {
                base.background(theme::BADGE_BG)
            } else {
                base
            }
        })
        .debug_name("review-launch-control");

    let popover_backend = backend.clone();
    Stack::new((
        button,
        dyn_container(
            move || state.review_launcher_open.get(),
            move |open| {
                if open {
                    launcher(app, state, popover_backend.clone()).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(31.0)
                .inset_right(0.0)
                .z_index(100)
        }),
    ))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Escape
            && key.modifiers.is_empty()
            && state.review_launcher_open.get_untracked()
        {
            close_launcher(state, true);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(|s| s.position(Position::Relative).flex_shrink(0.0_f32))
}

pub fn open_launcher(
    app: AppState,
    state: ReviewState,
    backend: &Backend,
    opener: Option<floem::ViewId>,
) {
    let Some(item_id) = state
        .item
        .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
    else {
        return;
    };
    state.review_launcher_opener.set(opener);
    state
        .review_launcher_draft
        .set(build_draft(app, state, &item_id));
    state.review_launcher_open.set(true);
    app.begin_review_dashboard_load(&item_id);
    backend.load_review_dashboard(&item_id);
}

fn close_launcher(state: ReviewState, restore_focus: bool) {
    state.review_launcher_open.set(false);
    if restore_focus {
        let opener = state.review_launcher_opener.get_untracked();
        state.review_launcher_opener.set(None);
        if let Some(opener) = opener {
            opener.request_focus();
        }
    } else {
        state.review_launcher_opener.set(None);
    }
}

fn launcher(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let close = icon_action(
        SidebarIcon::Close,
        "review-launch-close Close review launcher",
        move || close_launcher(state, true),
    );
    let close_id = close.view_id();
    let asked = Rc::new(Cell::new(false));
    let close = close.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.replace(true) {
            close_id.request_focus();
        }
    });

    let title = state
        .item
        .with_untracked(|item| item.as_ref().map(|item| item.title().to_string()))
        .unwrap_or_else(|| "branch".into());
    let rows_backend = backend.clone();
    let run_backend = backend.clone();
    let content = Stack::vertical((
        Stack::horizontal((
            Stack::vertical((
                Label::derived(|| "Review branch".to_string()).style(|s| {
                    s.font_size(theme::TEXT_BODY)
                        .font_weight(FontWeight::SEMI_BOLD)
                        .color(theme::FG)
                }),
                Label::derived(move || title.clone()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .color(theme::FG_SUBTLE)
                        .text_ellipsis()
                        .max_width(280.0)
                }),
            ))
            .style(|s| s.flex_col().gap(3.0).min_width(0.0).flex_grow(1.0_f32)),
            close,
        ))
        .style(|s| s.items_start().width_full().gap(8.0)),
        scope_note(state),
        dyn_container(
            move || (app.review_capabilities.get(), app.agent.get()),
            move |_| {
                Stack::vertical((
                    reviewer_row(
                        app,
                        state,
                        rows_backend.clone(),
                        perch_agent::Backend::ClaudeCode,
                    ),
                    reviewer_row(
                        app,
                        state,
                        rows_backend.clone(),
                        perch_agent::Backend::Codex,
                    ),
                ))
                .style(|s| s.flex_col().width_full().gap(10.0))
            },
        ),
        base_row(app, state),
        launch_error(app, state),
        Stack::horizontal((
            dyn_container(
                move || launch_validation(app, state).reason,
                |reason| match reason {
                    Some(reason) => Label::derived(move || reason.clone())
                        .style(|s| {
                            s.font_size(theme::TEXT_SECTION)
                                .color(theme::FG_SUBTLE)
                                .text_wrap()
                                .max_width(225.0)
                        })
                        .into_any(),
                    None => Empty::new().into_any(),
                },
            )
            .style(|s| s.min_width(0.0).flex_grow(1.0_f32)),
            action_button(
                "Run review",
                "run-review",
                true,
                move || launch_validation(app, state).request.is_some(),
                move || {
                    let validation = launch_validation(app, state);
                    let Some(request) = validation.request else {
                        return;
                    };
                    app.begin_review_dashboard_load(&request.item_id);
                    run_backend.start_review(request);
                    close_launcher(state, true);
                },
            ),
        ))
        .style(|s| s.items_center().width_full().gap(12.0)),
    ))
    .style(|s| s.flex_col().width_full().gap(14.0));

    Container::new(content)
        .style(|s| {
            s.width(390.0)
                .max_width_full()
                .padding(14.0)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(10.0)
                .box_shadow_blur(22.0)
                .box_shadow_v_offset(7.0)
                .box_shadow_color(theme::SHADOW)
        })
        .debug_name("review-launch-popover")
}

fn scope_note(state: ReviewState) -> impl IntoView {
    Label::derived(move || {
        if state.dirty.get().any() {
            "Committed changes only; local edits are excluded".to_string()
        } else {
            "Committed changes only".to_string()
        }
    })
    .style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::FG_SUBTLE)
            .width_full()
            .text_wrap()
    })
}

fn reviewer_row(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    reviewer: perch_agent::Backend,
) -> impl IntoView {
    let effort_backend = backend.clone();
    Stack::vertical((
        Stack::horizontal((
            Label::derived(move || backend_label(reviewer).to_string()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(FontWeight::MEDIUM)
                    .color(theme::FG)
                    .min_width(70.0)
            }),
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            dyn_container(
                move || effective_unavailable_reason(app, state, reviewer),
                move |reason| {
                    if reason.as_ref().is_some_and(|reason| {
                        capability(app, reviewer).is_none_or(|capability| !capability.available)
                            || reason.starts_with("No models")
                    }) {
                        return disabled_selector("None").into_any();
                    }
                    effort_selector(app, state, effort_backend.clone(), reviewer).into_any()
                },
            ),
        ))
        .style(|s| s.items_center().width_full().gap(8.0)),
        Stack::horizontal((
            Label::derived(|| "Model".to_string()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .color(theme::FG_SUBTLE)
                    .min_width(70.0)
            }),
            model_selector(app, state, backend, reviewer),
        ))
        .style(|s| s.items_center().width_full().gap(8.0)),
        dyn_container(
            move || effective_unavailable_reason(app, state, reviewer),
            |reason| match reason {
                Some(reason) => Label::derived(move || reason.clone())
                    .style(|s| {
                        s.font_size(theme::TEXT_SECTION)
                            .color(theme::FG_SUBTLE)
                            .text_wrap()
                            .width_full()
                    })
                    .into_any(),
                None => Empty::new().into_any(),
            },
        ),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(6.0)
            .padding(10.0)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
    })
    .debug_name(format!("reviewer-row:{}", reviewer.name()))
}

fn effort_selector(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    reviewer: perch_agent::Backend,
) -> impl IntoView {
    menu_button(
        move || {
            choice_for(state, reviewer)
                .filter(|choice| choice.enabled && effective_choice(app, choice).is_some())
                .and_then(|choice| choice.effort)
                .map(|effort| effort.label().to_string())
                .unwrap_or_else(|| "None".into())
        },
        move || {
            let selected = choice_for(state, reviewer);
            let none_selected = selected.clone();
            let none_backend = backend.clone();
            let mut menu = Menu::new().item("None", move |item| {
                item.checked(none_selected.as_ref().is_none_or(|choice| !choice.enabled))
                    .action({
                        let backend = none_backend.clone();
                        move || {
                            if let Some(choice) = update_choice(state, reviewer, |choice| {
                                choice.enabled = false;
                            }) {
                                backend.save_review_preference(choice);
                            }
                        }
                    })
            });
            let model = selected.and_then(|choice| choice.model);
            for effort in supported_efforts(app, reviewer, model.as_deref()) {
                let backend = backend.clone();
                let checked = choice_for(state, reviewer)
                    .is_some_and(|choice| choice.enabled && choice.effort == Some(effort));
                menu = menu.item(effort.label(), move |item| {
                    item.checked(checked).action(move || {
                        if let Some(choice) = update_choice(state, reviewer, |choice| {
                            choice.effort = Some(effort);
                            choice.enabled = true;
                        }) {
                            backend.save_review_preference(choice);
                        }
                    })
                });
            }
            menu
        },
        format!("review-effort:{}", reviewer.name()),
    )
}

fn model_selector(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    reviewer: perch_agent::Backend,
) -> impl IntoView {
    let models = models_for(&app.agent.get_untracked(), reviewer);
    let available = capability(app, reviewer).is_some_and(|capability| capability.available);
    if !available || models.is_empty() {
        return disabled_selector("Unavailable").into_any();
    }
    menu_button(
        move || {
            choice_for(state, reviewer)
                .and_then(|choice| choice.model)
                .unwrap_or_else(|| "Choose model".into())
        },
        move || {
            let selected = choice_for(state, reviewer).and_then(|choice| choice.model);
            let mut menu = Menu::new();
            for model in models.clone() {
                let backend = backend.clone();
                let pick = model.clone();
                let checked = selected.as_deref() == Some(model.as_str());
                menu = menu.item(model, move |item| {
                    let pick = pick.clone();
                    item.checked(checked).action(move || {
                        let wanted = choice_for(state, reviewer).and_then(|choice| choice.effort);
                        let effort = nearest_effort(app, reviewer, &pick, wanted);
                        if let Some(choice) = update_choice(state, reviewer, |choice| {
                            choice.model = Some(pick.clone());
                            choice.effort = effort;
                        }) {
                            backend.save_review_preference(choice);
                        }
                    })
                });
            }
            menu
        },
        format!("review-model:{}", reviewer.name()),
    )
    .into_any()
}

fn base_row(app: AppState, state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        Label::derived(|| "Base".to_string()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(FontWeight::MEDIUM)
                .color(theme::FG)
                .min_width(70.0)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        menu_button(
            move || {
                state
                    .review_launcher_draft
                    .with(|draft| draft.as_ref().and_then(|draft| draft.base_ref.clone()))
                    .unwrap_or_else(|| "Choose base".into())
            },
            move || {
                let Some(item_id) = current_item_id(state) else {
                    return Menu::new();
                };
                let selected = state.review_launcher_draft.with_untracked(|draft| {
                    draft.as_ref().and_then(|draft| draft.base_ref.clone())
                });
                let mut menu = Menu::new();
                for base_ref in app.review_base_refs_for(&item_id).iter() {
                    let pick = base_ref.clone();
                    let checked = selected.as_deref() == Some(base_ref.as_str());
                    menu = menu.item(base_ref.clone(), move |item| {
                        let pick = pick.clone();
                        item.checked(checked).action(move || {
                            state.review_launcher_draft.update(|draft| {
                                if let Some(draft) = draft {
                                    draft.base_ref = Some(pick.clone());
                                }
                            });
                        })
                    });
                }
                menu
            },
            "review-base".into(),
        ),
    ))
    .style(|s| s.items_center().width_full().gap(8.0))
}

fn launch_error(app: AppState, state: ReviewState) -> impl IntoView {
    dyn_container(
        move || {
            current_item_id(state).and_then(|item_id| {
                app.review_launch_errors
                    .with(|errors| errors.get(&item_id).cloned())
            })
        },
        |error| match error {
            Some(error) => Container::new(Label::derived(move || error.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::CLOSED)
                    .line_height(1.4)
                    .text_wrap()
                    .width_full()
            }))
            .style(|s| {
                s.width_full()
                    .padding(9.0)
                    .background(theme::CLOSED.with_alpha(0.07))
                    .border(1.0)
                    .border_color(theme::CLOSED.with_alpha(0.25))
                    .border_radius(theme::RADIUS)
            })
            .into_any(),
            None => Empty::new().into_any(),
        },
    )
}

struct LaunchValidation {
    request: Option<ReviewLaunchRequest>,
    reason: Option<String>,
}

fn launch_validation(app: AppState, state: ReviewState) -> LaunchValidation {
    let Some(item_id) = current_item_id(state) else {
        return invalid("No worktree is open");
    };
    if active_group(app, state).is_some() {
        return invalid("A review is already active for this worktree");
    }
    if app
        .review_dashboard_loading
        .with(|loading| loading.contains(&item_id))
    {
        return invalid("Checking review support…");
    }
    if app
        .review_launch_errors
        .with(|errors| errors.contains_key(&item_id))
    {
        return invalid("Review support could not be refreshed");
    }
    let Some(draft) = state.review_launcher_draft.get() else {
        return invalid("Review choices are still loading");
    };
    let Some(base_ref) = draft.base_ref.filter(|base| !base.trim().is_empty()) else {
        return invalid("Choose a local base ref");
    };
    let bases = app.review_base_refs_for(&item_id);
    if !bases.iter().any(|candidate| candidate == &base_ref) {
        return invalid(format!(
            "Base {base_ref} is not available locally; create or fetch it first"
        ));
    }
    let reviewers = draft
        .choices
        .iter()
        .filter_map(|choice| effective_choice(app, choice))
        .collect::<Vec<_>>();
    if reviewers.is_empty() {
        return invalid("Select at least one reviewer");
    }
    LaunchValidation {
        request: Some(ReviewLaunchRequest {
            item_id,
            base_ref,
            reviewers,
        }),
        reason: None,
    }
}

fn invalid(reason: impl Into<String>) -> LaunchValidation {
    LaunchValidation {
        request: None,
        reason: Some(reason.into()),
    }
}

/// Group-level summaries belong in Conversation even when a successful
/// review found nothing and therefore produced no finding threads.
pub fn history(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || {
            current_item_id(state).map(|item_id| {
                let groups = app.review_groups_for(&item_id);
                (item_id, groups)
            })
        },
        move |loaded| {
            let Some((_item_id, groups)) = loaded else {
                return Empty::new().into_any();
            };
            if groups.is_empty() {
                return Empty::new().into_any();
            }
            let opener = Rc::new(Cell::new(None));
            let action_opener = opener.clone();
            let latest_backend = backend.clone();
            let latest = action_button(
                "Review latest",
                "review-latest",
                false,
                move || active_group(app, state).is_none(),
                move || {
                    open_launcher(app, state, &latest_backend, action_opener.get());
                },
            );
            let latest_id = latest.view_id();
            opener.set(Some(latest_id));
            let cards_backend = backend.clone();
            Stack::vertical((
                Stack::horizontal((
                    Label::derived(|| "REVIEW HISTORY".to_string()).style(|s| {
                        s.font_size(theme::TEXT_SECTION)
                            .font_weight(FontWeight::SEMI_BOLD)
                            .color(theme::FG_SUBTLE)
                            .flex_grow(1.0_f32)
                    }),
                    latest,
                ))
                .style(|s| s.items_center().width_full()),
                dyn_stack(
                    move || groups.as_ref().clone(),
                    |summary| {
                        (
                            summary.group.id.clone(),
                            summary.group.status,
                            summary.group.new_findings,
                            summary.group.still_present,
                            summary.group.verified_fixed,
                            summary.group.not_verified,
                            summary
                                .attempts
                                .iter()
                                .map(|attempt| (attempt.id.clone(), attempt.phase))
                                .collect::<Vec<_>>(),
                        )
                    },
                    move |summary| group_card(app, state, cards_backend.clone(), summary),
                )
                .style(|s| s.flex_col().width_full().gap(10.0)),
            ))
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .gap(10.0)
                    .padding_top(18.0)
                    .padding_bottom(2.0)
                    .border_top(1.0)
                    .border_color(theme::BORDER_MUTED)
            })
            .debug_name("review-history")
            .into_any()
        },
    )
}

fn group_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    summary: ReviewGroupSummary,
) -> impl IntoView {
    let title = group_title(&summary);
    let attempts = attempt_summary(&summary);
    let counts = count_summary(&summary);
    let newer_summary = summary.clone();
    let open_summary = summary.clone();
    let open_backend = backend.clone();
    let openable = (summary.snapshot.manifest_path.is_some()
        && summary.snapshot.diff_path.is_some())
        || summary.group.status == perch_github::ReviewGroupStatus::Empty;
    let open = if openable {
        action_button(
            "Open snapshot",
            "open-review-snapshot",
            false,
            || true,
            move || {
                let request_id = open_backend.open_review_snapshot(
                    &open_summary.group.item_id,
                    &open_summary.group.id,
                );
                app.begin_historical_review(state, open_summary.clone(), request_id);
            },
        )
        .into_any()
    } else {
        Label::derived(|| "Snapshot unavailable".to_string())
            .style(|s| {
                s.height(28.0)
                    .items_center()
                    .font_size(theme::TEXT_BADGE)
                    .font_weight(FontWeight::MEDIUM)
                    .color(theme::FG_SUBTLE)
                    .selectable(false)
            })
            .into_any()
    };

    let group_id = summary.group.id.clone();
    let stop_backend = backend.clone();
    let active = summary.group.status.is_active();
    let stop = if active {
        action_button(
            "Stop all",
            "stop-review-group",
            false,
            || true,
            move || stop_backend.stop_review_group(&group_id),
        )
        .into_any()
    } else {
        Empty::new().into_any()
    };
    let retry_backend = backend.clone();
    let retries = summary
        .attempts
        .iter()
        .filter(|attempt| {
            matches!(
                attempt.phase,
                perch_github::ReviewerPhase::Failed
                    | perch_github::ReviewerPhase::Stopped
                    | perch_github::ReviewerPhase::Interrupted
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let retry = dyn_stack(
        move || retries.clone(),
        |attempt| attempt.id.clone(),
        move |attempt| {
            let id = attempt.id.clone();
            let label = format!("Retry {}", backend_label_from_name(&attempt.backend));
            let retry_backend = retry_backend.clone();
            action_button(
                &label,
                "retry-review-attempt",
                false,
                move || active_group(app, state).is_none(),
                move || retry_backend.retry_review_attempt(&id),
            )
        },
    )
    .style(|s| s.items_center().gap(4.0));

    Container::new(
        Stack::vertical((
            Stack::horizontal((
                Label::derived(move || title.clone()).style(|s| {
                    s.font_size(theme::TEXT_BODY)
                        .font_weight(FontWeight::SEMI_BOLD)
                        .color(theme::FG)
                        .text_wrap()
                        .min_width(0.0)
                        .flex_grow(1.0_f32)
                }),
                open,
            ))
            .style(|s| s.items_start().width_full().gap(8.0)),
            Label::derived(move || attempts.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_MUTED)
                    .text_wrap()
                    .width_full()
            }),
            Label::derived(move || counts.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .text_wrap()
                    .width_full()
            }),
            dyn_container(
                move || newer_changes(state, &newer_summary),
                |newer| {
                    if newer {
                        Label::derived(|| "Newer changes exist".to_string())
                            .style(|s| {
                                s.font_size(theme::TEXT_SECTION)
                                    .font_weight(FontWeight::MEDIUM)
                                    .color(theme::ACCENT)
                            })
                            .into_any()
                    } else {
                        Empty::new().into_any()
                    }
                },
            ),
            Stack::horizontal((retry, stop)).style(|s| s.items_center().gap(4.0)),
        ))
        .style(|s| s.flex_col().width_full().gap(7.0)),
    )
    .style(|s| {
        s.width_full()
            .padding(12.0)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(9.0)
    })
    .debug_name(format!("review-group:{}", summary.group.id))
}

fn build_draft(app: AppState, state: ReviewState, item_id: &str) -> Option<ReviewLaunchDraft> {
    let item = state
        .item
        .with_untracked(|item| item.as_ref().filter(|item| item.id == item_id).cloned())?;
    let agent = app.agent.get_untracked();
    let preferences = app.review_preferences.get_untracked();
    let has_preferences = !preferences.is_empty();
    let default_backend = crate::ask::backend_for(&agent, &agent.model);
    let fallback = perch_agent::Backend::ALL.into_iter().find(|backend| {
        capability(app, *backend).is_some_and(|capability| capability.available)
            && !models_for(&agent, *backend).is_empty()
    });
    let initially_enabled = if has_preferences {
        None
    } else if capability(app, default_backend).is_some_and(|capability| capability.available)
        && models_for(&agent, default_backend)
            .iter()
            .any(|model| model == &agent.model)
    {
        Some(default_backend)
    } else {
        fallback
    };

    let choices = perch_agent::Backend::ALL
        .into_iter()
        .map(|backend| {
            if let Some(preference) = preferences
                .iter()
                .find(|preference| preference.backend.eq_ignore_ascii_case(backend.name()))
            {
                let model = preference.model.clone();
                let effort = preference
                    .effort
                    .as_deref()
                    .and_then(perch_agent::ReviewEffort::parse);
                let mut choice = ReviewPreferenceChoice {
                    backend,
                    model,
                    effort,
                    enabled: preference.enabled,
                };
                if effective_choice(app, &choice).is_none() {
                    choice.enabled = false;
                }
                choice
            } else {
                let models = models_for(&agent, backend);
                let model = if backend == default_backend
                    && models.iter().any(|model| model == &agent.model)
                {
                    Some(agent.model.clone())
                } else {
                    models.first().cloned()
                };
                let effort = model.as_deref().and_then(|model| {
                    nearest_effort(
                        app,
                        backend,
                        model,
                        perch_agent::ReviewEffort::parse(&agent.effort),
                    )
                });
                ReviewPreferenceChoice {
                    backend,
                    model,
                    effort,
                    enabled: initially_enabled == Some(backend),
                }
            }
        })
        .collect();
    let base_ref = item
        .checkout
        .as_ref()
        .map(|checkout| checkout.base_ref.clone())
        .filter(|base| !base.trim().is_empty());
    Some(ReviewLaunchDraft {
        item_id: item_id.to_string(),
        base_ref,
        choices,
    })
}

fn current_item_id(state: ReviewState) -> Option<String> {
    state
        .item
        .with(|item| item.as_ref().map(|item| item.id.clone()))
}

fn choice_for(state: ReviewState, backend: perch_agent::Backend) -> Option<ReviewPreferenceChoice> {
    state.review_launcher_draft.with(|draft| {
        draft
            .as_ref()?
            .choices
            .iter()
            .find(|choice| choice.backend == backend)
            .cloned()
    })
}

fn update_choice(
    state: ReviewState,
    backend: perch_agent::Backend,
    update: impl FnOnce(&mut ReviewPreferenceChoice),
) -> Option<ReviewPreferenceChoice> {
    let mut updated = None;
    state.review_launcher_draft.update(|draft| {
        let Some(choice) = draft.as_mut().and_then(|draft| {
            draft
                .choices
                .iter_mut()
                .find(|choice| choice.backend == backend)
        }) else {
            return;
        };
        update(choice);
        updated = Some(choice.clone());
    });
    updated
}

fn capability(
    app: AppState,
    backend: perch_agent::Backend,
) -> Option<perch_agent::ReviewCapability> {
    app.review_capabilities.with(|capabilities| {
        capabilities
            .iter()
            .find(|capability| capability.backend == backend)
            .cloned()
    })
}

fn models_for(agent: &perch_github::AgentConfig, backend: perch_agent::Backend) -> Vec<String> {
    match backend {
        perch_agent::Backend::ClaudeCode => agent.models.clone(),
        perch_agent::Backend::Codex => agent.codex_models().to_vec(),
    }
}

fn supported_efforts(
    app: AppState,
    backend: perch_agent::Backend,
    model: Option<&str>,
) -> Vec<perch_agent::ReviewEffort> {
    let Some(model) = model else {
        return Vec::new();
    };
    let Some(capability) = capability(app, backend).filter(|capability| capability.available)
    else {
        return Vec::new();
    };
    let agent = app.agent.get_untracked();
    capability
        .efforts
        .into_iter()
        .filter(|effort| agent.effort_supported(model, effort.name()))
        .collect()
}

fn nearest_effort(
    app: AppState,
    backend: perch_agent::Backend,
    model: &str,
    wanted: Option<perch_agent::ReviewEffort>,
) -> Option<perch_agent::ReviewEffort> {
    let supported = supported_efforts(app, backend, Some(model));
    wanted
        .filter(|wanted| supported.contains(wanted))
        .or_else(|| supported.last().copied())
}

fn effective_choice(app: AppState, choice: &ReviewPreferenceChoice) -> Option<ReviewLaunchChoice> {
    if !choice.enabled {
        return None;
    }
    let capability = capability(app, choice.backend)?;
    if !capability.available {
        return None;
    }
    let model = choice.model.clone()?;
    if !models_for(&app.agent.get_untracked(), choice.backend).contains(&model) {
        return None;
    }
    let effort = choice.effort?;
    supported_efforts(app, choice.backend, Some(&model))
        .contains(&effort)
        .then_some(ReviewLaunchChoice {
            backend: choice.backend,
            model,
            effort,
        })
}

fn effective_unavailable_reason(
    app: AppState,
    state: ReviewState,
    backend: perch_agent::Backend,
) -> Option<String> {
    let Some(capability) = capability(app, backend) else {
        return Some("Checking installed CLI support…".into());
    };
    if !capability.available {
        return Some(
            capability
                .reason
                .unwrap_or_else(|| "Native review is unavailable".into()),
        );
    }
    let models = models_for(&app.agent.get_untracked(), backend);
    if models.is_empty() {
        return Some(format!(
            "No models are configured for {}",
            backend_label(backend)
        ));
    }
    let choice = choice_for(state, backend)?;
    if let Some(model) = choice.model.as_deref() {
        if !models.iter().any(|candidate| candidate == model) {
            return Some(format!(
                "Saved model {model} is unavailable; choose another model"
            ));
        }
        if choice.enabled
            && choice.effort.is_none_or(|effort| {
                !supported_efforts(app, backend, Some(model)).contains(&effort)
            })
        {
            return Some("Choose a supported local effort".into());
        }
    } else {
        return Some("Choose a model".into());
    }
    None
}

fn active_group(app: AppState, state: ReviewState) -> Option<ReviewGroupSummary> {
    let item_id = current_item_id(state)?;
    app.review_groups_for(&item_id)
        .iter()
        .find(|summary| summary.group.status.is_active())
        .cloned()
}

fn progress_label(summary: &ReviewGroupSummary) -> String {
    let total = summary.attempts.len();
    let complete = summary
        .attempts
        .iter()
        .filter(|attempt| attempt.phase.is_terminal())
        .count();
    format!("Reviewing · {complete} of {total} complete")
}

fn open_agents(app: AppState, state: ReviewState, backend: &Backend, opener: floem::ViewId) {
    state.review_launcher_open.set(false);
    app.agents_opener.set(Some(opener));
    app.agents_tab.set(AgentsTab::Active);
    app.agents_worktree_filter.set(WorktreeFilter::Current);
    app.agents_backend_filter.set(BackendFilter::All);
    app.selected_agent_run.set(None);
    app.agents_open.set(true);
    backend.set_meta("agents.open", "true");
}

fn group_title(summary: &ReviewGroupSummary) -> String {
    format!(
        "Review #{} · {} · against {} at {}",
        summary.group.number,
        short_sha(&summary.snapshot.head_sha),
        summary.snapshot.requested_base_ref,
        short_sha(&summary.snapshot.base_tip_sha)
    )
}

fn attempt_summary(summary: &ReviewGroupSummary) -> String {
    if summary.attempts.is_empty() {
        return "No reviewer attempts were admitted".into();
    }
    summary
        .attempts
        .iter()
        .map(|attempt| {
            format!(
                "{}: {}",
                backend_label_from_name(&attempt.backend),
                phase_label(attempt.phase)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn count_summary(summary: &ReviewGroupSummary) -> String {
    format!(
        "{} new · {} still present · {} verified fixed · {} not verified",
        summary.group.new_findings,
        summary.group.still_present,
        summary.group.verified_fixed,
        summary.group.not_verified
    )
}

fn newer_changes(state: ReviewState, summary: &ReviewGroupSummary) -> bool {
    state.item.with(|item| {
        item.as_ref()
            .and_then(|item| item.checkout.as_ref())
            .is_some_and(|checkout| checkout.head_sha != summary.snapshot.head_sha)
    })
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn backend_label(backend: perch_agent::Backend) -> &'static str {
    match backend {
        perch_agent::Backend::ClaudeCode => "Claude",
        perch_agent::Backend::Codex => "Codex",
    }
}

fn backend_label_from_name(backend: &str) -> &'static str {
    perch_agent::Backend::parse(backend)
        .map(backend_label)
        .unwrap_or("Reviewer")
}

fn phase_label(phase: perch_github::ReviewerPhase) -> &'static str {
    match phase {
        perch_github::ReviewerPhase::Queued => "queued",
        perch_github::ReviewerPhase::Preparing => "preparing",
        perch_github::ReviewerPhase::Reviewing => "reviewing",
        perch_github::ReviewerPhase::Extracting => "extracting",
        perch_github::ReviewerPhase::Verifying => "verifying",
        perch_github::ReviewerPhase::Reconciling => "reconciling",
        perch_github::ReviewerPhase::Stopping => "stopping",
        perch_github::ReviewerPhase::Completed => "completed",
        perch_github::ReviewerPhase::Failed => "failed",
        perch_github::ReviewerPhase::Stopped => "stopped",
        perch_github::ReviewerPhase::Interrupted => "interrupted",
    }
}

fn menu_button(
    label: impl Fn() -> String + 'static,
    menu: impl Fn() -> Menu + 'static,
    debug: String,
) -> impl View {
    Stack::horizontal((
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG)
                .selectable(false)
                .pointer_events_none()
                .margin_right(5.0)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE)
            .style(|s| s.size(9.0, 9.0).pointer_events_none()),
    ))
    .popout_menu(menu)
    .style(|s| {
        clickable(s)
            .items_center()
            .height(26.0)
            .padding_horiz(8.0)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
    .debug_name(debug)
    .into_view()
}

fn disabled_selector(label: &'static str) -> impl IntoView {
    Label::derived(move || label.to_string()).style(|s| {
        s.height(26.0)
            .items_center()
            .padding_horiz(8.0)
            .font_size(theme::TEXT_BADGE)
            .color(theme::FG_SUBTLE)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
            .selectable(false)
    })
}

fn action_button(
    label: &str,
    debug: &'static str,
    prominent: bool,
    enabled: impl Fn() -> bool + 'static,
    action: impl Fn() + 'static,
) -> impl View {
    let label = label.to_string();
    let enabled: Rc<dyn Fn() -> bool> = Rc::new(enabled);
    let event_enabled = enabled.clone();
    let style_enabled = enabled.clone();
    let action: Rc<dyn Fn()> = Rc::new(action);
    let click = action.clone();
    Container::new(Label::derived(move || label.clone()).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(if prominent {
                theme::CANVAS
            } else {
                theme::FG_MUTED
            })
            .selectable(false)
            .pointer_events_none()
    }))
    .on_event_stop(el::Click, move |_, _| {
        if event_enabled() {
            click();
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) && enabled() {
            action();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .style(move |s| {
        let available = style_enabled();
        clickable(s)
            .height(28.0)
            .padding_horiz(10.0)
            .items_center()
            .justify_center()
            .background(if prominent { theme::FG } else { theme::CANVAS })
            .border(1.0)
            .border_color(if prominent { theme::FG } else { theme::BORDER })
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::ACCENT_RING))
            .apply_if(!available, |s| {
                s.opacity(0.45_f32).cursor(CursorStyle::Default)
            })
            .apply_if(available, |s| {
                s.hover(move |s| {
                    s.background(if prominent {
                        theme::ACTION_HOVER
                    } else {
                        theme::CANVAS_HOVER
                    })
                })
            })
    })
    .debug_name(debug)
    .into_view()
}

fn icon_action(icon: SidebarIcon, debug: &'static str, action: impl Fn() + 'static) -> impl View {
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

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::{Key, KeyState, Modifiers, NamedKey};
    use jiff::Timestamp;

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn key(code: Code, key: Key) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    fn available(backend: perch_agent::Backend) -> perch_agent::ReviewCapability {
        perch_agent::ReviewCapability {
            backend,
            version: Some(perch_agent::CliVersion::new(9, 9, 9)),
            available: true,
            efforts: perch_agent::ReviewEffort::ALL.to_vec(),
            reason: None,
            adapter_version: perch_agent::REVIEW_ADAPTER_VERSION,
        }
    }

    fn item_state() -> (AppState, ReviewState) {
        let app = AppState::new();
        let state = ReviewState::new();
        let mut item = perch_github::Item::new(
            "local/perch",
            "2026-09-19T12:00:00Z".parse::<Timestamp>().unwrap(),
        );
        item.id = "worktree".into();
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "feature/reviews".into(),
            base_ref: "main".into(),
            head_sha: "abcdef0123456789".into(),
            path: std::path::PathBuf::from("/tmp/perch-review-ui-test"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        state.item.set(Some(std::sync::Arc::new(item)));
        app.review_capabilities.set(std::sync::Arc::new(vec![
            available(perch_agent::Backend::ClaudeCode),
            available(perch_agent::Backend::Codex),
        ]));
        app.review_base_refs.update(|refs| {
            refs.insert("worktree".into(), std::sync::Arc::new(vec!["main".into()]));
        });
        (app, state)
    }

    fn summary(status: perch_github::ReviewGroupStatus) -> ReviewGroupSummary {
        let created_at = "2026-09-19T12:00:00Z".parse::<Timestamp>().unwrap();
        ReviewGroupSummary {
            group: perch_github::ReviewGroup {
                id: "group-3".into(),
                item_id: "worktree".into(),
                snapshot_id: "snapshot-3".into(),
                number: 3,
                status,
                new_findings: 2,
                still_present: 3,
                verified_fixed: 1,
                not_verified: 1,
                created_at,
                started_at: Some(created_at),
                ended_at: status.is_terminal().then_some(created_at),
            },
            snapshot: perch_github::ReviewSnapshot {
                id: "snapshot-3".into(),
                item_id: "worktree".into(),
                repository: "local/perch".into(),
                branch_label: Some("feature/reviews".into()),
                requested_base_ref: "main".into(),
                base_tip_sha: "1234567890abcdef".into(),
                merge_base_sha: "1234567890abcdef".into(),
                head_sha: "abcdef0123456789".into(),
                dirty_edits_excluded: false,
                retention_key: "retained-3".into(),
                retained_ref: Some("refs/perch/reviews/snapshot-3".into()),
                manifest_path: Some("manifest.json".into()),
                diff_path: Some("review.diff".into()),
                created_at,
            },
            attempts: Vec::new(),
        }
    }

    #[test]
    fn first_use_enables_only_the_default_models_backend() {
        let (app, state) = item_state();
        let draft = build_draft(app, state, "worktree").unwrap();
        let claude = draft
            .choices
            .iter()
            .find(|choice| choice.backend == perch_agent::Backend::ClaudeCode)
            .unwrap();
        let codex = draft
            .choices
            .iter()
            .find(|choice| choice.backend == perch_agent::Backend::Codex)
            .unwrap();
        assert!(claude.enabled);
        assert_eq!(claude.model.as_deref(), Some("sonnet"));
        assert_eq!(claude.effort, Some(perch_agent::ReviewEffort::Medium));
        assert!(!codex.enabled);
    }

    #[test]
    fn unavailable_saved_choice_is_preserved_but_not_effective() {
        let (app, state) = item_state();
        app.review_preferences
            .set(std::sync::Arc::new(vec![perch_github::ReviewPreference {
                backend: "claude-code".into(),
                model: Some("retired-model".into()),
                effort: Some("high".into()),
                enabled: true,
                updated_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            }]));
        let draft = build_draft(app, state, "worktree").unwrap();
        let claude = draft
            .choices
            .iter()
            .find(|choice| choice.backend == perch_agent::Backend::ClaudeCode)
            .unwrap();
        assert_eq!(claude.model.as_deref(), Some("retired-model"));
        assert_eq!(claude.effort, Some(perch_agent::ReviewEffort::High));
        assert!(!claude.enabled);
        assert!(effective_choice(app, claude).is_none());
    }

    #[test]
    fn an_unknown_external_base_requires_an_explicit_selection() {
        let (app, state) = item_state();
        state.item.update(|item| {
            std::sync::Arc::make_mut(item.as_mut().unwrap())
                .checkout
                .as_mut()
                .unwrap()
                .base_ref
                .clear();
        });
        let draft = build_draft(app, state, "worktree").unwrap();
        assert_eq!(draft.base_ref, None);
        state.review_launcher_draft.set(Some(draft));
        assert_eq!(
            launch_validation(app, state).reason.as_deref(),
            Some("Choose a local base ref")
        );
    }

    #[test]
    fn reviewer_choices_are_independent_and_both_none_cannot_launch() {
        let (app, state) = item_state();
        let mut agent = app.agent.get_untracked();
        agent.codex.models = vec!["gpt-5.5".into()];
        app.agent.set(agent);
        state
            .review_launcher_draft
            .set(build_draft(app, state, "worktree"));

        update_choice(state, perch_agent::Backend::Codex, |choice| {
            choice.enabled = true;
            choice.effort = Some(perch_agent::ReviewEffort::High);
        });
        let both = launch_validation(app, state).request.unwrap();
        assert_eq!(both.reviewers.len(), 2);

        state.review_launcher_draft.update(|draft| {
            for choice in &mut draft.as_mut().unwrap().choices {
                choice.enabled = false;
            }
        });
        let neither = launch_validation(app, state);
        assert!(neither.request.is_none());
        assert_eq!(
            neither.reason.as_deref(),
            Some("Select at least one reviewer")
        );
    }

    #[test]
    fn zero_attempt_review_still_has_a_history_card() {
        let (app, state) = item_state();
        app.update_review_group(
            "worktree".into(),
            summary(perch_github::ReviewGroupStatus::Empty),
        );
        let root = TestRoot::new();
        let view = history(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 700.0, 500.0);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "review-history").is_some());
        assert!(named(harness.root_id(), "review-group:group-3").is_some());
    }

    #[test]
    fn launcher_moves_focus_in_and_escape_returns_it_to_the_control() {
        let (app, state) = item_state();
        let root = TestRoot::new();
        let view = control(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 700.0, 500.0);
        harness.rebuild();
        harness.paint();
        let control = named(harness.root_id(), "review-launch-control").unwrap();
        control.request_focus();
        harness.rebuild();
        assert!(harness.is_focused(control));

        harness.dispatch_event(key(Code::Enter, Key::Named(NamedKey::Enter)));
        harness.rebuild();
        harness.paint();
        assert!(state.review_launcher_open.get());
        let close = named(harness.root_id(), "review-launch-close").unwrap();
        assert!(harness.is_focused(close));

        harness.dispatch_event(key(Code::Escape, Key::Named(NamedKey::Escape)));
        harness.rebuild();
        harness.paint();
        assert!(!state.review_launcher_open.get());
        assert!(harness.is_focused(control));
    }
}
