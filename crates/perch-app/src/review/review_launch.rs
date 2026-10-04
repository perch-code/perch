//! Native branch-review launch, progress, and retained review history.
//!
//! Findings themselves remain ordinary `CommentThread`s. This module owns
//! only the group-level controls and summaries around those threads.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::model::{
    ReviewGroupSummary, ReviewLaunchChoice, ReviewLaunchDraft, ReviewLaunchRequest,
    ReviewPreferenceChoice, ReviewState,
};
use crate::state::AppState;
use crate::theme;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::widgets::{clickable, editor_box, prose_editor, sidebar_glyph};

/// Keep the open item's reviews loaded, and the launcher's choices in step
/// with what the reviewers support while the Review popover is open.
pub(crate) fn watch(app: AppState, state: ReviewState, backend: Rc<Backend>) {
    // History and an in-flight group's progress belong in the worktree view,
    // not behind the popover's first click. Reload when the open item
    // changes; opening the popover performs an explicit refresh.
    Effect::new(move |previous: Option<Option<String>>| {
        let item_id = current_item_id(state);
        if previous.as_ref() != Some(&item_id) {
            if let Some(item_id) = item_id.as_deref() {
                app.begin_review_dashboard_load(item_id);
                backend.load_review_dashboard(item_id);
            }
        }
        item_id
    });

    Effect::new(move |_| {
        let open = state.review_open.get();
        let _capabilities = app.review_capabilities.get();
        let _preferences = app.review_preferences.get();
        let _agent = app.agent.get();
        let item_id = state
            .item
            .with(|item| item.as_ref().map(|item| item.id.clone()));
        let _base_refs = item_id
            .as_deref()
            .map(|item_id| app.review_base_refs_for(item_id));
        if open {
            if let Some(item_id) = item_id {
                state
                    .review_launcher_draft
                    .set(build_draft(app, state, &item_id));
            }
        }
    });
}

/// The agents' part of the Review popover. Over the reader's own review
/// on a pull request it is one line, the reviewers chosen and Run, with
/// Settings to unfold the rest, which folds the reader's review in turn;
/// on a branch it is all there is, unfolded.
/// Under the line, what the agents are doing or last found.
pub(crate) fn agents_section(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    submitting: Memo<bool>,
) -> impl IntoView {
    let expanded = Memo::new(move |_| !submitting.get() || state.review_agents_expanded.get());
    let rows_backend = backend.clone();
    let run_backend = backend.clone();
    let run: Rc<dyn Fn()> = Rc::new(move || {
        let validation = launch_validation(app, state);
        let Some(request) = validation.request else {
            return;
        };
        app.begin_review_launch(&request.item_id);
        run_backend.start_review(request);
        crate::review::review_menu::close(state, true);
    });
    let head_run = run.clone();
    let foot_run = run.clone();

    let head = Stack::horizontal((
        Label::derived(|| "Agents".to_string()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || reviewers_summary(app, state))
            .debug_name("review-agents-summary")
            .style(move |s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .apply_if(expanded.get(), |s| s.hide())
            }),
        Empty::new().style(move |s| s.flex_grow(1.0_f32).apply_if(!expanded.get(), |s| s.hide())),
        dyn_container(
            move || submitting.get(),
            move |submitting| {
                if !submitting {
                    return Empty::new().into_any();
                }
                Container::new(
                    Label::derived(move || {
                        if state.review_agents_expanded.get() {
                            "Hide".to_string()
                        } else {
                            "Settings".to_string()
                        }
                    })
                    .style(|s| {
                        s.font_size(theme::TEXT_BADGE)
                            .color(theme::fg_muted())
                            .selectable(false)
                            .pointer_events_none()
                    }),
                )
                .on_event_stop(el::Click, move |_, _| {
                    state.review_agents_expanded.update(|open| *open = !*open)
                })
                .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
                    if matches!(key.code, Code::Enter | Code::Space) {
                        state.review_agents_expanded.update(|open| *open = !*open);
                        EventPropagation::Stop
                    } else {
                        EventPropagation::Continue
                    }
                })
                .style(|s| {
                    clickable(s)
                        .height(28.0)
                        .items_center()
                        .padding_horiz(8.0)
                        .border_radius(theme::RADIUS)
                        .keyboard_navigable()
                        .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
                        .hover(|s| s.background(theme::canvas_hover()))
                })
                .debug_name("review-agents-toggle")
                .into_any()
            },
        ),
        dyn_container(
            move || expanded.get(),
            move |expanded| {
                if expanded {
                    return Empty::new().into_any();
                }
                let run = head_run.clone();
                action_button(
                    "Run",
                    "run-review",
                    false,
                    move || launch_validation(app, state).request.is_some(),
                    move || run(),
                )
                .into_any()
            },
        ),
    ))
    .style(|s| s.items_center().width_full().gap(8.0));

    let settings = Stack::vertical((
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
        head_row(state),
        instructions_box(state, run),
        launch_error(app, state),
        Stack::horizontal((
            dyn_container(
                move || launch_validation(app, state).reason,
                |reason| match reason {
                    Some(reason) => Label::derived(move || reason.clone())
                        .style(|s| {
                            s.font_size(theme::TEXT_SECTION)
                                .color(theme::fg_subtle())
                                .text_wrap()
                                .max_width(225.0)
                        })
                        .into_any(),
                    None => Empty::new().into_any(),
                },
            )
            .style(|s| s.min_width(0.0).flex_grow(1.0_f32)),
            // Unfolded, the settings are all the popover shows, so Run
            // review is its one action.
            action_button(
                "Run review",
                "run-review",
                true,
                move || launch_validation(app, state).request.is_some(),
                move || foot_run(),
            ),
        ))
        .style(|s| s.items_center().width_full().gap(12.0)),
    ))
    .style(move |s| {
        s.flex_col()
            .width_full()
            .gap(14.0)
            .apply_if(!expanded.get(), |s| s.hide())
    });

    Stack::vertical((head, agents_status(app, state, expanded), settings))
        .style(|s| s.flex_col().width_full().gap(8.0))
        .debug_name("review-agents")
}

/// The reviewers the next run would start, `Claude Opus (high)`, one
/// after another; or why there are none.
fn reviewers_summary(app: AppState, state: ReviewState) -> String {
    let chosen = state.review_launcher_draft.with(|draft| {
        draft
            .as_ref()
            .map(|draft| {
                draft
                    .choices
                    .iter()
                    .filter_map(|choice| effective_choice(app, choice))
                    .map(|choice| {
                        crate::review::threads::agent_name(
                            choice.backend.name(),
                            &choice.model,
                            choice.effort.name(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    if chosen.is_empty() {
        "No reviewer chosen".to_string()
    } else {
        chosen.join(", ")
    }
}

/// What the agents are doing, with Show to go to them in the Agents section;
/// else, folded, why Run cannot go; else what the last review found.
fn agents_status(app: AppState, state: ReviewState, expanded: Memo<bool>) -> impl IntoView {
    #[derive(Clone, PartialEq)]
    enum Status {
        Active(String),
        Note(String),
        Nothing,
    }
    let status = move || {
        if let Some(group) = active_group(app, state) {
            return Status::Active(progress_label(&group));
        }
        if !expanded.get() {
            if let Some(reason) = launch_validation(app, state).reason {
                return Status::Note(reason);
            }
        }
        let latest = current_item_id(state).and_then(|item_id| {
            app.review_groups_for(&item_id)
                .iter()
                .max_by_key(|summary| summary.group.number)
                .cloned()
        });
        match latest {
            Some(summary) => Status::Note(format!(
                "Review #{} \u{b7} {}",
                summary.group.number,
                count_summary(&summary)
            )),
            None => Status::Nothing,
        }
    };
    dyn_container(status, move |status| {
        let text = |text: String| {
            Label::derived(move || text.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .text_wrap()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
            })
        };
        match status {
            Status::Active(progress) => Stack::horizontal((
                text(progress),
                Container::new(Label::derived(|| "Show".to_string()).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::accent())
                        .selectable(false)
                        .pointer_events_none()
                }))
                .on_event_stop(el::Click, move |_, _| {
                    if let Some(group) = active_group(app, state) {
                        show_in_agents(app, state, &group);
                    }
                })
                .style(|s| {
                    clickable(s)
                        .padding_horiz(6.0)
                        .padding_vert(3.0)
                        .border_radius(theme::RADIUS)
                        .hover(|s| s.background(theme::canvas_hover()))
                })
                .debug_name("review-show-agents"),
            ))
            .style(|s| s.items_center().width_full().gap(8.0))
            .debug_name("review-agents-status")
            .into_any(),
            Status::Note(note) => text(note).debug_name("review-agents-status").into_any(),
            Status::Nothing => Empty::new().into_any(),
        }
    })
    .style(|s| s.width_full())
}

fn scope_note(state: ReviewState) -> impl IntoView {
    Label::derived(move || {
        if includes_uncommitted(state) {
            "Commits and uncommitted work, untracked files included".to_string()
        } else if state.dirty.get().any() {
            "Committed changes only; local edits are excluded".to_string()
        } else {
            "Committed changes only".to_string()
        }
    })
    .style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg_subtle())
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
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
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
                    .color(theme::fg_subtle())
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
                            .color(theme::fg_subtle())
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
            .background(theme::canvas_subtle())
            .border(1.0)
            .border_color(theme::border_muted())
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
                .map(|model| crate::review::threads::model_label(&model))
                .unwrap_or_else(|| "Choose model".into())
        },
        move || {
            let selected = choice_for(state, reviewer).and_then(|choice| choice.model);
            let mut menu = Menu::new();
            for model in models.clone() {
                let backend = backend.clone();
                let pick = model.clone();
                let checked = selected.as_deref() == Some(model.as_str());
                menu = menu.item(crate::review::threads::model_label(&model), move |item| {
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
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
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

/// Why the working tree cannot be the head, or `None` when it can: it
/// has to be on disk, have work in it, and sit on the head the item has.
fn uncommitted_unavailable(state: ReviewState) -> Option<&'static str> {
    let on_disk = state.item.with(|item| {
        item.as_ref()
            .and_then(|item| item.checkout.as_ref())
            .is_some_and(|checkout| checkout.on_disk)
    });
    if !on_disk {
        Some("not on disk")
    } else if state.stale_head.with(Option::is_some) {
        Some("head moved")
    } else if !state.dirty.get().any() {
        Some("clean")
    } else {
        None
    }
}

/// Whether the next review reads the uncommitted work as well as the
/// commits: the reader's pick, else yes when that work is what the Files
/// view shows, or is all the branch has.
fn includes_uncommitted(state: ReviewState) -> bool {
    if uncommitted_unavailable(state).is_some() {
        return false;
    }
    state.review_launcher_uncommitted.get().unwrap_or_else(|| {
        matches!(
            state.scope.get(),
            perch_git::Scope::Uncommitted | perch_git::Scope::Unstaged | perch_git::Scope::Staged
        ) || state.commits.with(|commits| commits.is_empty())
    })
}

/// Where the reviewed range ends: the last commit, or the working tree
/// over it.
fn head_row(state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        Label::derived(|| "Head".to_string()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .min_width(70.0)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        menu_button(
            move || {
                if includes_uncommitted(state) {
                    "Working tree".into()
                } else {
                    "Last commit".into()
                }
            },
            move || {
                let included = includes_uncommitted(state);
                let unavailable = uncommitted_unavailable(state);
                let pick = move |uncommitted: bool| {
                    move || state.review_launcher_uncommitted.set(Some(uncommitted))
                };
                let working_tree = match unavailable {
                    Some(reason) => format!("Working tree ({reason})"),
                    None => "Working tree".to_string(),
                };
                Menu::new()
                    .item("Last commit", move |item| {
                        item.checked(!included).action(pick(false))
                    })
                    .item(working_tree, move |item| {
                        item.checked(included)
                            .enabled(unavailable.is_none())
                            .action(pick(true))
                    })
            },
            "review-head".into(),
        ),
    ))
    .style(|s| s.items_center().width_full().gap(8.0))
}

/// What the reviewers should focus on, such as one file of a branch that
/// carries more. Cmd-Enter runs the review and Escape closes the popover,
/// as in the summary box.
fn instructions_box(state: ReviewState, run: Rc<dyn Fn()>) -> impl IntoView {
    use floem::views::editor::command::CommandExecuted;

    use crate::review::threads::cmd_enter;

    let editor = prose_editor(
        state.review_launcher_instructions.get_untracked(),
        "Instructions for the reviewers (optional)",
        None,
    );
    let editor = editor
        .pre_command(move |cmd| {
            if cmd_enter(&cmd) {
                run();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        .update(move |update| {
            if let Some(editor) = update.editor {
                state
                    .review_launcher_instructions
                    .set(editor.doc().text().to_string());
            }
        })
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |_, key: &KeyboardEvent| {
                if key.code == Code::Escape && key.modifiers.is_empty() {
                    crate::review::review_menu::cancel(state, true);
                    EventPropagation::Stop
                } else {
                    EventPropagation::Continue
                }
            },
        )
        .style(|s| {
            s.size_full()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_META)
        })
        .debug_name("review-instructions");
    editor_box(editor, || (64.0, 160.0), true)
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
                    .color(theme::closed())
                    .line_height(theme::PROSE_LINE_HEIGHT)
                    .text_wrap()
                    .width_full()
            }))
            .style(|s| {
                s.width_full()
                    .padding(9.0)
                    .background(theme::closed().with_alpha(0.07))
                    .border(1.0)
                    .border_color(theme::closed().with_alpha(0.25))
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
    if app.is_review_launching(&item_id) {
        return invalid("Starting the review…");
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
    let instructions = state.review_launcher_instructions.with_untracked(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    });
    LaunchValidation {
        request: Some(ReviewLaunchRequest {
            item_id,
            base_ref,
            reviewers,
            instructions,
            include_uncommitted: includes_uncommitted(state),
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
/// A review's findings, a chip for each kind it has: new in the notice's
/// yellow, fixed in green, and the rest grey. A kind with none has no
/// chip.
pub(crate) fn count_chips(summary: &ReviewGroupSummary) -> impl IntoView {
    type Ink = fn() -> floem::peniko::Color;
    let group = &summary.group;
    let kinds: [(u64, &str, Ink, Ink); 4] = [
        (group.new_findings, "new", theme::notice_fg, theme::notice_bg),
        (group.still_present, "still present", theme::fg_muted, theme::badge_bg),
        (group.verified_fixed, "verified fixed", theme::open, theme::diff_added_bg),
        (group.not_verified, "not verified", theme::fg_subtle, theme::badge_bg),
    ];
    let chips = kinds
        .into_iter()
        .filter(|(count, ..)| *count > 0)
        .map(|(count, words, ink, fill)| {
            crate::ui::widgets::badge(move || format!("{count} {words}"), ink, fill, None).into_any()
        })
        .collect::<Vec<_>>();
    Stack::horizontal_from_iter(chips)
        .debug_name("review-group-counts")
        .style(|s| s.items_center().flex_wrap(floem::style::FlexWrap::Wrap).gap(6.0))
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

pub(crate) fn active_group(app: AppState, state: ReviewState) -> Option<ReviewGroupSummary> {
    let item_id = current_item_id(state)?;
    app.review_groups_for(&item_id)
        .iter()
        .find(|summary| summary.group.status.is_active())
        .cloned()
}

/// Run was pressed on the open item and its review has not arrived yet.
pub(crate) fn launching(app: AppState, state: ReviewState) -> bool {
    current_item_id(state).is_some_and(|item_id| app.is_review_launching(&item_id))
}

pub(crate) fn progress_label(summary: &ReviewGroupSummary) -> String {
    let total = summary.attempts.len();
    let complete = summary
        .attempts
        .iter()
        .filter(|attempt| attempt.phase.is_terminal())
        .count();
    format!("Reviewing · {complete} of {total} complete")
}

/// While a review runs, Show in the Review popover goes to it in the
/// item's Agents section: the section unfolds, and the review's first attempt with
/// a run opens at the top of the page.
fn show_in_agents(app: AppState, state: ReviewState, summary: &ReviewGroupSummary) {
    crate::review::review_menu::close(state, false);
    let run = app.agent_runs.with_untracked(|runs| {
        runs.iter()
            .find(|run| {
                run.review_attempt_id
                    .as_deref()
                    .is_some_and(|attempt| summary.attempts.iter().any(|a| a.id == attempt))
            })
            .map(|run| run.id.clone())
    });
    floem::reactive::Effect::batch(|| {
        state.show_tab(crate::review::model::ItemTab::Conversation);
        state.agents_unfolded.set(true);
        if let Some(run) = run {
            state.pending_run_reveal.set(Some(run));
        }
    });
}


pub(crate) fn attempt_summary(summary: &ReviewGroupSummary) -> String {
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

/// What a finished attempt left unread: its counts cover only what Perch
/// could read, so an incomplete ingestion must not pass for a quiet review.
pub(crate) fn unread_summary(summary: &ReviewGroupSummary) -> Option<String> {
    let lines = summary
        .attempts
        .iter()
        .filter(|attempt| {
            attempt.phase == perch_github::ReviewerPhase::Completed
                && attempt.ingestion_state == perch_github::ReviewIngestionState::Incomplete
        })
        .map(|attempt| {
            let backend = backend_label_from_name(&attempt.backend);
            match attempt
                .diagnostics
                .as_deref()
                .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
            {
                Some(reason) => format!("Perch could not read all of {backend}'s output: {reason}"),
                None => format!("Perch could not read all of {backend}'s output"),
            }
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
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

/// Whether the checkout has moved past what a review read. A review of
/// uncommitted work is judged by the commit it sat on; later edits to the
/// work itself do not show here.
pub(crate) fn newer_changes(state: ReviewState, summary: &ReviewGroupSummary) -> bool {
    let reviewed = summary
        .snapshot
        .worktree_parent_sha
        .as_deref()
        .unwrap_or(&summary.snapshot.head_sha);
    state.item.with(|item| {
        item.as_ref()
            .and_then(|item| item.checkout.as_ref())
            .is_some_and(|checkout| checkout.head_sha != reviewed)
    })
}


fn backend_label(backend: perch_agent::Backend) -> &'static str {
    match backend {
        perch_agent::Backend::ClaudeCode => "Claude",
        perch_agent::Backend::Codex => "Codex",
    }
}

pub(crate) fn backend_label_from_name(backend: &str) -> &'static str {
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

pub(crate) fn menu_button(
    label: impl Fn() -> String + 'static,
    menu: impl Fn() -> Menu + 'static,
    debug: String,
) -> impl View {
    Stack::horizontal((
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg())
                .selectable(false)
                .pointer_events_none()
                .margin_right(5.0)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_subtle)
            .style(|s| s.size(9.0, 9.0).pointer_events_none()),
    ))
    .dropdown(menu)
    .style(|s| {
        clickable(s)
            .items_center()
            .height(26.0)
            .padding_horiz(8.0)
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .hover(|s| s.background(theme::canvas_hover()))
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
            .color(theme::fg_subtle())
            .background(theme::canvas_subtle())
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(theme::RADIUS)
            .selectable(false)
    })
}

pub(crate) fn action_button(
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
            .font_weight(theme::weight_emphasis())
            .color(if prominent {
                theme::canvas()
            } else {
                theme::fg_muted()
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
            .background(if prominent { theme::fg() } else { theme::canvas() })
            .border(1.0)
            .border_color(if prominent { theme::fg() } else { theme::border() })
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .apply_if(!available, |s| {
                s.opacity(0.45_f32).cursor(CursorStyle::Default)
            })
            .apply_if(available, |s| {
                s.hover(move |s| {
                    s.background(if prominent {
                        theme::action_hover()
                    } else {
                        theme::canvas_hover()
                    })
                })
            })
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
            divergence: None,
            source: None,
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
                instructions: None,
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
                worktree_parent_sha: None,
            },
            attempts: Vec::new(),
        }
    }

    #[test]
    fn a_started_review_says_so_until_its_group_arrives_or_fails() {
        use perch_github::ReviewGroupStatus;

        let (app, state) = item_state();
        app.update_review_group("worktree".into(), summary(ReviewGroupStatus::Completed));
        app.begin_review_launch("worktree");
        assert!(launching(app, state));
        assert_eq!(
            launch_validation(app, state).reason.as_deref(),
            Some("Starting the review…")
        );

        // News of an older group is not the new one arriving.
        app.update_review_group("worktree".into(), summary(ReviewGroupStatus::Completed));
        assert!(launching(app, state));

        let mut started = summary(ReviewGroupStatus::Preparing);
        started.group.id = "group-4".into();
        started.group.number = 4;
        app.update_review_group("worktree".into(), started);
        assert!(!launching(app, state));
        assert!(active_group(app, state).is_some());

        app.begin_review_launch("worktree");
        app.fail_review_launch("worktree".into(), "Could not capture".into());
        assert!(!launching(app, state));
    }

    #[test]
    fn an_unread_output_is_called_out_rather_than_passing_for_no_findings() {
        let created_at = "2026-09-19T12:00:00Z".parse::<Timestamp>().unwrap();
        let attempt =
            |backend: &str,
             ingestion_state: perch_github::ReviewIngestionState,
             diagnostics: Option<&str>| perch_github::ReviewerAttempt {
                id: format!("attempt-{backend}"),
                group_id: "group-3".into(),
                run_id: None,
                backend: backend.into(),
                executable: None,
                executable_version: None,
                model: "model".into(),
                effort: "max".into(),
                adapter_version: "3".into(),
                output_format_version: "1".into(),
                attempt_number: 1,
                retry_of: None,
                phase: perch_github::ReviewerPhase::Completed,
                ingestion_state,
                result_kind: None,
                output_id: None,
                raw_result: None,
                diagnostics: diagnostics.map(str::to_string),
                verification_input: perch_github::ReviewVerificationInput::default(),
                output_saved_at: Some(created_at),
                created_at,
                started_at: Some(created_at),
                ended_at: Some(created_at),
            };
        let mut review = summary(perch_github::ReviewGroupStatus::Completed);
        review.attempts = vec![attempt(
            "claude-code",
            perch_github::ReviewIngestionState::Complete,
            Some("finding 3 was rejected"),
        )];
        assert_eq!(unread_summary(&review), None);

        review.attempts = vec![
            attempt(
                "claude-code",
                perch_github::ReviewIngestionState::Incomplete,
                Some("\nthe printed findings array was invalid\nmore detail"),
            ),
            attempt(
                "codex",
                perch_github::ReviewIngestionState::Incomplete,
                None,
            ),
        ];
        assert_eq!(
            unread_summary(&review).as_deref(),
            Some(
                "Perch could not read all of Claude's output: the printed findings array was invalid\nPerch could not read all of Codex's output"
            )
        );
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
        assert_eq!(claude.model.as_deref(), Some("opus"));
        assert_eq!(claude.effort, Some(perch_agent::ReviewEffort::High));
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
    fn instructions_ride_along_trimmed_and_an_empty_box_sends_none() {
        let (app, state) = item_state();
        state
            .review_launcher_draft
            .set(build_draft(app, state, "worktree"));
        assert_eq!(launch_validation(app, state).request.unwrap().instructions, None);

        state.review_launcher_instructions.set("  \n ".into());
        assert_eq!(launch_validation(app, state).request.unwrap().instructions, None);

        state
            .review_launcher_instructions
            .set("\nReview rfcs/multi_get.md only.\n".into());
        assert_eq!(
            launch_validation(app, state)
                .request
                .unwrap()
                .instructions
                .as_deref(),
            Some("Review rfcs/multi_get.md only.")
        );

        // A reviewer's support changing rebuilds the draft, not the box.
        state
            .review_launcher_draft
            .set(build_draft(app, state, "worktree"));
        assert!(launch_validation(app, state).request.unwrap().instructions.is_some());
    }

    #[test]
    fn uncommitted_work_is_reviewed_when_it_is_what_the_reader_sees() {
        let (app, state) = item_state();
        state
            .review_launcher_draft
            .set(build_draft(app, state, "worktree"));
        let included = || launch_validation(app, state).request.unwrap().include_uncommitted;
        let commit = perch_git::Commit {
            sha: "abcdef0123456789".into(),
            subject: "Start".into(),
            body: String::new(),
            author: "Chris".into(),
            authored_at: "2026-09-19T12:00:00Z".parse().unwrap(),
        };

        // A clean worktree has only its commits to give.
        assert!(!included());
        state.review_launcher_uncommitted.set(Some(true));
        assert!(!included(), "a clean worktree cannot be picked");
        state.review_launcher_uncommitted.set(None);

        // With nothing committed, the work is all there is.
        state.dirty.set(perch_git::Dirt { unstaged: true, staged: false });
        assert!(included());

        // Over commits it follows the Files view, until the reader picks.
        state.commits.set(std::sync::Arc::new(vec![commit]));
        assert!(!included());
        state.scope.set(perch_git::Scope::Uncommitted);
        assert!(included());
        state.review_launcher_uncommitted.set(Some(false));
        assert!(!included());
        state.scope.set(perch_git::Scope::Branch);
        state.review_launcher_uncommitted.set(Some(true));
        assert!(included());

        // A worktree left on an older head of its pull request is not.
        state.stale_head.set(Some("newer".into()));
        assert!(!included());
        assert_eq!(uncommitted_unavailable(state), Some("head moved"));
    }

    #[test]
    fn launcher_moves_focus_in_and_escape_returns_it_to_the_control() {
        let (app, state) = item_state();
        let root = TestRoot::new();
        let view =
            crate::review::review_menu::control(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 700.0, 800.0);
        harness.rebuild();
        harness.paint();
        let control = named(harness.root_id(), "review-control").unwrap();
        control.request_focus();
        harness.rebuild();
        assert!(harness.is_focused(control));

        harness.dispatch_event(key(Code::Enter, Key::Named(NamedKey::Enter)));
        harness.rebuild();
        harness.paint();
        assert!(state.review_open.get());
        let close = named(harness.root_id(), "review-close").unwrap();
        assert!(harness.is_focused(close));
        assert!(named(harness.root_id(), "review-instructions").is_some());

        harness.dispatch_event(key(Code::Escape, Key::Named(NamedKey::Escape)));
        harness.rebuild();
        harness.paint();
        assert!(!state.review_open.get());
        assert!(harness.is_focused(control));
    }
}
