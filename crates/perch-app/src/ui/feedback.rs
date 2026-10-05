//! The bug icon in the bar, and Send Feedback's dropdown under it: a bug
//! report or an idea, filed as an issue on Perch's own repository.
//!
//! The dropdown has three pages. The form takes a title, what happened,
//! Bug or Idea, and whether an agent should look into it first. Preview…
//! has the backend collect what Perch knows about itself, run the agent
//! and search for similar issues, while the second page says how far it
//! has got. The third is the issue as it will be sent, editable, with
//! File issue, and +1 or Comment on an issue that is already there.
//!
//! The draft is the app's, not the window's, and outlives the dropdown:
//! a press outside or Escape shuts it and the icon brings it back on the
//! page it was on. Only sending, or Discard, ends it.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, ScrollExt, Stack, TextInput};
use floem::AnyView;
use jiff::Timestamp;

use crate::backend::{Backend, Event, Status};
use crate::feedback::{
    FeedbackDraft, FeedbackKind, Pending, PlusOne, PrepareRequest, Problem, SimilarIssue, Step,
    WindowFacts, REPO,
};
use crate::icons::{self, SidebarIcon};
use crate::review::cards::tip;
use crate::review::model::{AgentChoice, ReviewState};
use crate::review::timeline::count_of;
use crate::state::{AppState, Checked, ToastOpen, Tone};
use crate::theme;
use crate::ui::widgets::{
    action_button, checkbox, clickable, editor_box, icon_in, prose_editor, sidebar_glyph,
};

/// The dropdown's three pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Form,
    Working,
    Preview,
}

fn page_of(step: Step) -> Page {
    match step {
        Step::Editing => Page::Form,
        Step::Collecting | Step::Diagnosing | Step::Searching => Page::Working,
        Step::Preview | Step::Filing => Page::Preview,
    }
}

/// The bug icon in the drawer's footer, and the dropdown over it while it
/// is open.
pub fn button(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let color = move || theme::nav_muted();
    let button = Container::new(
        icon_in(
            move || icons::sidebar_icon(SidebarIcon::Bug, color()),
            color,
        )
        .style(|s| s.pointer_events_none()),
    );
    let button_id = button.view_id();
    let button = button
        .on_event_stop(el::Click, move |_, _| toggle(app, review, Some(button_id)))
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                toggle(app, review, Some(button_id));
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .style(move |s| crate::ui::sidebar::footer_button(s, review.feedback_open.get()))
        .debug_name("feedback-button");

    let stack = Stack::new((
        tip(button, "Report a bug".to_string()),
        dyn_container(
            move || review.feedback_open.get(),
            move |open| {
                if open {
                    dropdown(app, review, backend.clone()).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(crate::ui::sidebar::footer_popover),
    ));
    // A press outside the button and what hangs from it shuts it: see
    // `ui::dismiss_popovers`.
    let anchor = stack.view_id();
    Effect::new(move |_| {
        if review.feedback_open.get() {
            review.feedback_anchor.set(Some(anchor));
        }
    });
    stack
        .style(|s| {
            s.position(Position::Relative)
                .flex_shrink(0.0_f32)
                .pointer_events_auto()
        })
}

/// Open the dropdown, or shut it when it is open.
pub fn toggle(app: AppState, review: ReviewState, opener: Option<floem::ViewId>) {
    if review.feedback_open.get_untracked() {
        close(review, true);
    } else {
        open(app, review, FeedbackKind::Bug, opener);
    }
}

/// Open the dropdown over whatever else hangs from the bar, on the draft
/// there is, or a new one of `kind`. It hangs from the drawer's footer, so
/// a shut drawer opens with it.
pub fn open(app: AppState, review: ReviewState, kind: FeedbackKind, opener: Option<floem::ViewId>) {
    app.sidebar_open.set(true);
    crate::review::review_menu::close(review, false);
    crate::ui::agents::dropdown::close(review, false);
    crate::ui::usage::close(review, false);
    crate::ui::invite::close(review, false);
    if app.feedback.with_untracked(Option::is_none) {
        app.feedback.set(Some(FeedbackDraft::new(kind)));
    }
    review.feedback_opener.set(opener);
    review.feedback_open.set(true);
}

/// Shut the dropdown, keeping the draft, and give the keyboard back to
/// what had it when it opened when `restore_focus`.
pub fn close(review: ReviewState, restore_focus: bool) {
    if !review.feedback_open.get_untracked() {
        return;
    }
    review.feedback_open.set(false);
    let opener = review.feedback_opener.get_untracked();
    review.feedback_opener.set(None);
    if restore_focus {
        if let Some(opener) = opener {
            opener.request_focus();
        }
    }
}

/// Report on a toast: a Bug about it. A draft with something in it isn't
/// replaced without asking.
pub fn report(app: AppState, review: ReviewState, text: String, tone: Tone) {
    // The toast's own line in the session's list, so Diagnostics marks it.
    let problem = app
        .recent_problems
        .with_untracked(|problems| problems.iter().rev().find(|p| p.text == text).cloned())
        .unwrap_or(Problem {
            at: Timestamp::now(),
            text,
            tone,
        });
    let busy = app
        .feedback
        .with_untracked(|draft| draft.as_ref().is_some_and(|d| d.has_content()));
    if busy {
        app.feedback.update(|draft| {
            if let Some(draft) = draft {
                draft.offer = Some(problem);
            }
        });
    } else {
        replace(app, FeedbackDraft::from_problem(problem));
    }
    open(app, review, FeedbackKind::Bug, None);
}

/// The backend's answers about the draft, in the window that asked.
pub fn apply(app: AppState, review: ReviewState, event: Event) {
    match event {
        Event::FeedbackStep { attempt, step } => app.feedback.update(|draft| {
            if let Some(draft) = draft.as_mut().filter(|d| d.attempt == attempt) {
                if page_of(draft.step) == Page::Working {
                    draft.step = step;
                }
            }
        }),
        Event::FeedbackReady(prepared) => {
            let mut taken = false;
            app.feedback.update(|draft| {
                let Some(draft) = draft.as_mut().filter(|d| d.attempt == prepared.attempt) else {
                    return;
                };
                if page_of(draft.step) != Page::Working {
                    return;
                }
                let prepared = *prepared;
                draft.context = Some(prepared.context);
                draft.notes = prepared.notes;
                draft.notes_error = prepared.notes_error;
                draft.preview = prepared.preview;
                draft.redacted = prepared.redacted;
                draft.similar = prepared.similar;
                draft.step = Step::Preview;
                draft.generation += 1;
                taken = true;
            });
            if taken && !review.feedback_open.get_untracked() {
                app.notify_with(
                    "Your report is ready to review",
                    Tone::Success,
                    Some(ToastOpen::FeedbackPreview),
                );
            }
        }
        Event::FeedbackSent { number, outcome } => {
            app.feedback.set(None);
            close(review, false);
            let (text, tone) = outcome.toast(number);
            app.notify_with(
                text,
                tone,
                Some(ToastOpen::Subject {
                    repo_full_name: REPO.to_string(),
                    kind: perch_github::ItemKind::Issue,
                    number,
                }),
            );
        }
        Event::FeedbackCaught {
            title,
            preview,
            count,
        } => app.feedback.update(|draft| {
            if let Some(draft) = draft {
                draft.title = title;
                draft.preview = preview;
                draft.caught_on_send = count;
                draft.pending = None;
                draft.step = Step::Preview;
                draft.generation += 1;
            }
        }),
        Event::FeedbackFailed(error) => app.feedback.update(|draft| {
            if let Some(draft) = draft {
                draft.error = Some(error);
                draft.pending = None;
                if draft.step == Step::Filing {
                    draft.step = Step::Preview;
                }
            }
        }),
        _ => {}
    }
}

/// The model menu's pick, or the CLI there is when the pick's isn't
/// installed. None with neither.
fn diagnosis_choice(app: AppState, review: ReviewState) -> Option<AgentChoice> {
    let (claude, codex) = app
        .agent
        .with(|agent| (!agent.models.is_empty(), !agent.codex.models.is_empty()));
    let picked = review
        .agent_last
        .get()
        .unwrap_or_else(|| crate::review::threads::defaults(app));
    let has = |backend| match backend {
        perch_agent::Backend::ClaudeCode => claude,
        perch_agent::Backend::Codex => codex,
    };
    if has(picked.0) {
        return Some(picked);
    }
    let other = match picked.0 {
        perch_agent::Backend::ClaudeCode => perch_agent::Backend::Codex,
        perch_agent::Backend::Codex => perch_agent::Backend::ClaudeCode,
    };
    if !has(other) {
        return None;
    }
    app.agent.with(|agent| {
        let model = match other {
            perch_agent::Backend::ClaudeCode => agent.models.first().cloned(),
            perch_agent::Backend::Codex => agent.codex.models.first().cloned(),
        }?;
        Some((other, model, picked.2))
    })
}

fn agent_name(choice: &Option<AgentChoice>) -> &'static str {
    match choice.as_ref().map(|c| c.0) {
        Some(perch_agent::Backend::Codex) => "Codex",
        _ => "Claude",
    }
}

/// Preview…: hand the backend the draft and what the window knows.
fn prepare(app: AppState, review: ReviewState, backend: &Backend) {
    let Some(mut draft) = app.feedback.get_untracked() else {
        return;
    };
    if draft.title.trim().is_empty() {
        return;
    }
    draft.attempt += 1;
    draft.step = Step::Collecting;
    draft.error = None;
    draft.caught_on_send = 0;
    draft.offer = None;
    let choice = diagnosis_choice(app, review);
    let request = PrepareRequest {
        attempt: draft.attempt,
        kind: draft.kind,
        title: draft.title.trim().to_string(),
        body: draft.body.clone(),
        diagnose: draft.diagnose && choice.is_some(),
        seed: draft.seed.clone(),
        problems: app
            .recent_problems
            .with_untracked(|p| p.iter().cloned().collect()),
        item_id: review
            .item
            .with_untracked(|i| i.as_ref().map(|i| i.id.clone())),
        choice,
        facts: window_facts(app, review),
    };
    app.feedback.set(Some(draft));
    backend.prepare_feedback(request);
}

fn window_facts(app: AppState, review: ReviewState) -> WindowFacts {
    let item_open = review.item.with_untracked(Option::is_some);
    let last_poll = match app.status.get_untracked() {
        Status::Idle { at, .. } => Some(format!(
            "fine, {}",
            crate::timefmt::relative(at, Timestamp::now())
        )),
        Status::Refreshing => Some("in progress".to_string()),
        Status::Error { message } => Some(format!("failed: {message}")),
        Status::RateLimited { message } => Some(format!("rate limited: {message}")),
        Status::NoToken { .. } => Some("no token".to_string()),
    };
    WindowFacts {
        display: crate::windows::display_facts(),
        language_server: item_open.then(|| review.lsp_status.get_untracked().describe()),
        last_poll,
    }
}

/// Put a new draft in place of the old one, drawing the page again so its
/// fields hold the new text.
fn replace(app: AppState, mut draft: FeedbackDraft) {
    // Past the old draft's attempt too, so an answer still on its way for
    // the old one can't land on this one.
    let (generation, attempt) = app.feedback.with_untracked(|d| {
        d.as_ref()
            .map_or((0, 0), |d| (d.generation + 1, d.attempt + 1))
    });
    draft.generation = draft.generation.max(generation);
    draft.attempt = draft.attempt.max(attempt);
    app.feedback.set(Some(draft));
}

fn update(app: AppState, f: impl FnOnce(&mut FeedbackDraft)) {
    app.feedback.update(|draft| {
        if let Some(draft) = draft {
            f(draft);
        }
    });
}

fn draft_with<T>(app: AppState, f: impl FnOnce(&FeedbackDraft) -> T) -> Option<T> {
    app.feedback.with(|draft| draft.as_ref().map(f))
}

/// The dropdown: a page by the draft's step, rebuilt when the page or
/// the text put in from outside changes, not on every keystroke.
fn dropdown(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let shown = Memo::new(move |_| draft_with(app, |d| (page_of(d.step), d.generation)));
    Container::new(
        dyn_container(
            move || shown.get(),
            move |shown| -> AnyView {
                match shown {
                    None => Empty::new().into_any(),
                    Some((Page::Form, _)) => form(app, review, backend.clone()).into_any(),
                    Some((Page::Working, _)) => working(app, review, backend.clone()).into_any(),
                    Some((Page::Preview, _)) => preview(app, review, backend.clone()).into_any(),
                }
            },
        )
        .style(|s| s.width_full()),
    )
    .style(|s| {
        s.width(theme::FOOTER_POPOVER_WIDTH)
            .flex_col()
            .apply(crate::ui::menu::panel_surface(floem::style::Style::new()))
    })
    .debug_name("feedback-dropdown")
}

/// A page: its body, which scrolls when the dropdown would run too
/// tall, over its buttons, which stay put under a rule.
fn frame(
    app: AppState,
    body: Vec<AnyView>,
    footer: impl IntoView + 'static,
    debug: &'static str,
) -> impl IntoView {
    Stack::vertical((
        Stack::vertical_from_iter(body)
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .gap(12.0)
                    .padding(16.0)
                    .padding_bottom(14.0)
            })
            .scroll()
            .style(move |s| {
                s.width_full().max_height(
                    crate::ui::sidebar::footer_popover_room(app, theme::FEEDBACK_DROPDOWN_MAX_HEIGHT)
                        - FOOTER_HEIGHT,
                )
            }),
        Container::new(footer).style(|s| {
            s.width_full()
                .padding_horiz(16.0)
                .padding_vert(12.0)
                .border_top(1.0)
                .border_color(theme::border_muted())
        }),
    ))
    .style(|s| s.flex_col().width_full())
    .debug_name(debug)
}

/// About what a footer of one row of buttons takes.
const FOOTER_HEIGHT: f64 = 56.0;

/// The page's top: a glyph, a title, a line under it, and whatever goes
/// at the right.
fn header(
    title: impl Fn() -> String + 'static,
    subtitle: impl Fn() -> String + 'static,
    trailing: impl IntoView + 'static,
) -> impl IntoView {
    Stack::vertical((
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Bug, theme::fg)
                .style(|s| s.size(15.0, 15.0).flex_shrink(0.0_f32)),
            Label::derived(title).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_strong())
                    .color(theme::fg())
                    .selectable(false)
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
            }),
            trailing,
        ))
        .style(|s| s.items_center().gap(8.0).width_full()),
        Label::derived(subtitle).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .text_wrap()
                .width_full()
                .selectable(false)
        }),
    ))
    .style(|s| s.flex_col().gap(4.0).width_full())
}

/// Bug · Idea.
fn kind_switch(app: AppState) -> impl IntoView {
    let segment = move |kind: FeedbackKind| {
        let current = move || draft_with(app, |d| d.kind == kind).unwrap_or(false);
        Label::derived(move || kind.label().to_string())
            .on_event_stop(el::Click, move |_, _| {
                // The placeholders are the kind's, so the form is drawn
                // again with them.
                update(app, |d| {
                    if d.kind != kind {
                        d.set_kind(kind);
                        d.generation += 1;
                    }
                })
            })
            .style(move |s| {
                let on = current();
                clickable(s)
                    .height(24.0)
                    .padding_horiz(10.0)
                    .items_center()
                    .font_size(theme::TEXT_BADGE)
                    .border_radius(6.0)
                    .color(if on { theme::fg() } else { theme::fg_muted() })
                    .apply_if(on, |s| {
                        s.background(theme::canvas())
                            .font_weight(theme::weight_emphasis())
                            .box_shadow_blur(2.0)
                            .box_shadow_v_offset(1.0)
                            .box_shadow_color(theme::shadow())
                    })
                    .apply_if(!on, |s| s.hover(|s| s.color(theme::fg())))
            })
            .debug_name(format!("feedback-kind-{}", kind.label()))
    };
    Stack::horizontal((segment(FeedbackKind::Bug), segment(FeedbackKind::Idea))).style(|s| {
        s.items_center()
            .padding(2.0)
            .gap(2.0)
            .flex_shrink(0.0_f32)
            .background(theme::canvas_subtle())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
    })
}

fn title_input(
    app: AppState,
    placeholder: &'static str,
    disabled: impl Fn() -> bool + 'static,
) -> impl IntoView {
    let title = RwSignal::new(draft_with(app, |d| d.title.clone()).unwrap_or_default());
    // Only the field is followed: text put into the draft from outside
    // rebuilds the page, and must not be written over by the old field.
    Effect::new(move |_| {
        let text = title.get();
        let changed = app
            .feedback
            .with_untracked(|d| d.as_ref().is_some_and(|d| d.title != text));
        if changed {
            update(app, |d| d.title = text);
        }
    });
    TextInput::new(title)
        .placeholder(placeholder)
        .debug_name("feedback-title")
        .style(move |s| {
            s.width_full()
                .height(32.0)
                .font_size(theme::TEXT_BODY)
                .padding_horiz(10.0)
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::accent()))
                .set_disabled(disabled())
        })
}

/// A box of Markdown, styled as a composer's, that writes back into the
/// draft as it's typed.
fn text_box(
    review: ReviewState,
    initial: String,
    placeholder: String,
    height: (f64, f64),
    debug: &'static str,
    disabled: impl Fn() -> bool + 'static,
    write: impl Fn(String) + 'static,
) -> impl IntoView {
    let editor = prose_editor(initial, placeholder, Some(review.next_token()))
        .update(move |update| {
            if let Some(editor) = update.editor {
                write(editor.doc().text().to_string());
            }
        })
        .style(move |s| {
            s.size_full()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_BODY)
                .set_disabled(disabled())
        })
        .debug_name(debug);
    editor_box(editor, move || height, true)
}

/// A muted line with a dot: the toast being reported, or a note.
fn note(
    color: impl theme::Ink,
    text: impl Fn() -> String + 'static,
    debug: &'static str,
) -> impl IntoView {
    Stack::horizontal((
        crate::ui::toast::dot(color).style(|s| s.margin_top(4.0)),
        Label::derived(text).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .text_wrap()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
    ))
    .debug_name(debug)
    .style(|s| {
        s.items_start()
            .gap(8.0)
            .width_full()
            .padding_horiz(10.0)
            .padding_vert(8.0)
            .background(theme::canvas_subtle())
            .border_radius(theme::RADIUS)
    })
}

/// The form: what happened, and Preview….
fn form(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let kind = move || draft_with(app, |d| d.kind).unwrap_or(FeedbackKind::Bug);
    let choice = move || diagnosis_choice(app, review);
    let can_preview = move || draft_with(app, |d| !d.title.trim().is_empty()).unwrap_or(false);
    let preview_backend = backend.clone();
    let start =
        draft_with(app, Clone::clone).unwrap_or_else(|| FeedbackDraft::new(FeedbackKind::Bug));

    let seed = start.seed.clone().map(|seed| {
        let color: fn() -> floem::peniko::Color = match seed.tone {
            Tone::Error => theme::closed,
            _ => theme::draft_yellow,
        };
        let text = seed.text.lines().next().unwrap_or("").to_string();
        note(color, move || format!("Reporting: {text}"), "feedback-seed").into_any()
    });

    let offer = dyn_container(
        move || draft_with(app, |d| d.offer.clone()).flatten(),
        move |offer| -> AnyView {
            let Some(problem) = offer else {
                return Empty::new().into_any();
            };
            Stack::horizontal((
                Label::derived(|| "You've started a report. Replace it with this one?".to_string())
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::fg())
                            .text_wrap()
                            .min_width(0.0)
                            .flex_grow(1.0_f32)
                            .flex_basis(0.0)
                    }),
                action_button(
                    || "Keep mine".to_string(),
                    "feedback-offer-keep",
                    false,
                    || true,
                    move || update(app, |d| d.offer = None),
                ),
                action_button(
                    || "Replace".to_string(),
                    "feedback-offer-replace",
                    false,
                    || true,
                    move || replace(app, FeedbackDraft::from_problem(problem.clone())),
                ),
            ))
            .style(|s| {
                s.items_center()
                    .gap(8.0)
                    .width_full()
                    .padding(10.0)
                    .background(theme::notice_bg())
                    .border(1.0)
                    .border_color(theme::notice_border())
                    .border_radius(theme::RADIUS)
            })
            .into_any()
        },
    )
    .style(move |s| {
        s.width_full().apply_if(
            draft_with(app, |d| d.offer.is_none()).unwrap_or(true),
            |s| s.hide(),
        )
    });

    let body = text_box(
        review,
        start.body.clone(),
        start.kind.placeholder().to_string(),
        (120.0, 240.0),
        "feedback-body",
        || false,
        move |text| {
            if draft_with(app, |d| d.body != text).unwrap_or(false) {
                update(app, |d| d.body = text);
            }
        },
    );

    let toggle = move || {
        if choice().is_some() {
            update(app, |d| d.diagnose = !d.diagnose);
        }
    };
    let click = toggle;
    let diagnose = Stack::horizontal((
        checkbox(
            move || {
                if choice().is_some() && draft_with(app, |d| d.diagnose).unwrap_or(false) {
                    Checked::Full
                } else {
                    Checked::Empty
                }
            },
            toggle,
        )
        .style(|s| s.margin_top(-4.0)),
        Stack::vertical((
            Label::derived(move || format!("Let {} look into it", agent_name(&choice()))).style(
                |s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                        .selectable(false)
                },
            ),
            Label::derived(move || {
                if choice().is_none() {
                    "Needs Claude Code or Codex installed.".to_string()
                } else {
                    "It reads Perch's logs and runs read-only commands on this Mac, offline. \
                     Its notes go in the preview."
                        .to_string()
                }
            })
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .text_wrap()
                    .width_full()
                    .selectable(false)
            }),
        ))
        .style(|s| {
            s.flex_col()
                .gap(2.0)
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| click())
    .debug_name("feedback-diagnose")
    .style(move |s| {
        clickable(s)
            .items_start()
            .gap(4.0)
            .width_full()
            .apply_if(choice().is_none(), |s| s.opacity(0.55_f32))
    });

    let discard_shown = move || draft_with(app, FeedbackDraft::has_content).unwrap_or(false);
    let footer = Stack::horizontal((
        action_button(
            || "Discard".to_string(),
            "feedback-discard",
            false,
            || true,
            move || {
                app.feedback.set(None);
                close(review, true);
            },
        )
        .style(move |s| s.apply_if(!discard_shown(), |s| s.hide())),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        action_button(
            || "Preview…".to_string(),
            "feedback-preview",
            true,
            can_preview,
            move || prepare(app, review, &preview_backend),
        ),
    ))
    .style(|s| s.items_center().gap(8.0).width_full());

    let placeholder = match start.kind {
        FeedbackKind::Bug => "What went wrong, in a line",
        FeedbackKind::Idea => "Your idea, in a line",
    };
    let enter_backend = backend.clone();
    let mut parts: Vec<AnyView> = vec![
        header(
            move || match kind() {
                FeedbackKind::Bug => "Report a bug".to_string(),
                FeedbackKind::Idea => "Suggest an idea".to_string(),
            },
            || format!("Becomes an issue on {REPO}, after you've seen it."),
            kind_switch(app),
        )
        .into_any(),
        offer.into_any(),
    ];
    parts.extend(seed);
    parts.extend([
        title_input(app, placeholder, || false).into_any(),
        body.into_any(),
        diagnose.into_any(),
    ]);
    frame(app, parts, footer, "feedback-form").on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Enter && key.modifiers.meta() && can_preview() {
            prepare(app, review, &enter_backend);
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
}

/// Preview… on its way: each step, done, under way or to come.
fn working(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let step = move || draft_with(app, |d| d.step).unwrap_or(Step::Collecting);
    let diagnosing = move || {
        draft_with(app, |d| d.diagnose).unwrap_or(false) && diagnosis_choice(app, review).is_some()
    };
    let name = agent_name(&diagnosis_choice(app, review));
    let order = |s: Step| match s {
        Step::Collecting => 0,
        Step::Diagnosing => 1,
        Step::Searching => 2,
        _ => 3,
    };
    let row = move |at: Step, todo: String, doing: String, done: String| {
        let state = move || {
            let now = order(step());
            let mine = order(at);
            (mine < now, mine == now)
        };
        let (todo, doing, done) = (Rc::new(todo), Rc::new(doing), Rc::new(done));
        Stack::horizontal((
            dyn_container(state, |(finished, current)| -> AnyView {
                if finished {
                    sidebar_glyph(SidebarIcon::Check, theme::open)
                        .style(|s| s.size(14.0, 14.0))
                        .into_any()
                } else if current {
                    Empty::new()
                        .style(|s| {
                            s.size(10.0, 10.0)
                                .margin(2.0)
                                .border_radius(5.0)
                                .background(theme::accent())
                        })
                        .into_any()
                } else {
                    Empty::new()
                        .style(|s| {
                            s.size(10.0, 10.0)
                                .margin(2.0)
                                .border_radius(5.0)
                                .border(1.5)
                                .border_color(theme::border())
                        })
                        .into_any()
                }
            })
            .style(|s| {
                s.size(14.0, 14.0)
                    .items_center()
                    .justify_center()
                    .flex_shrink(0.0_f32)
            }),
            Label::derived(move || {
                let (finished, current) = state();
                if finished {
                    (*done).clone()
                } else if current {
                    (*doing).clone()
                } else {
                    (*todo).clone()
                }
            })
            .style(move |s| {
                let (finished, current) = state();
                s.font_size(theme::TEXT_META)
                    .selectable(false)
                    .color(if current {
                        theme::fg()
                    } else if finished {
                        theme::fg_muted()
                    } else {
                        theme::fg_subtle()
                    })
                    .apply_if(current, |s| s.font_weight(theme::weight_emphasis()))
            }),
        ))
        .style(|s| s.items_center().gap(10.0).height(22.0))
    };
    let stop_backend = backend.clone();
    let back_backend = backend.clone();
    let steps = Stack::vertical((
        row(
            Step::Collecting,
            "Collect diagnostics".into(),
            "Collecting diagnostics…".into(),
            "Collected diagnostics".into(),
        ),
        row(
            Step::Diagnosing,
            format!("{name} looks into it"),
            format!("{name} is looking into it…"),
            format!("{name} looked into it"),
        )
        .style(move |s| s.apply_if(!diagnosing(), |s| s.hide())),
        row(
            Step::Searching,
            "Search for similar issues".into(),
            "Searching for similar issues…".into(),
            "Searched for similar issues".into(),
        ),
    ))
    .style(|s| {
        s.flex_col()
            .gap(6.0)
            .width_full()
            .padding(12.0)
            .background(theme::canvas_subtle())
            .border_radius(theme::RADIUS)
    })
    .debug_name("feedback-steps");
    let hint = Label::derived(move || {
        format!(
            "{name} can take a minute or two. Close this and keep working; Perch will tell you \
                 when your report is ready."
        )
    })
    .style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::fg_subtle())
            .text_wrap()
            .width_full()
            .selectable(false)
            .apply_if(step() != Step::Diagnosing, |s| s.hide())
    });
    let footer = Stack::horizontal((
        action_button(
            || "Back".to_string(),
            "feedback-working-back",
            false,
            || true,
            move || {
                back_backend.stop_feedback_diagnosis();
                update(app, FeedbackDraft::back_to_form);
            },
        ),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        action_button(
            move || format!("Skip {name}"),
            "feedback-stop",
            false,
            || true,
            move || stop_backend.stop_feedback_diagnosis(),
        )
        .style(move |s| s.apply_if(step() != Step::Diagnosing, |s| s.hide())),
        action_button(
            || "Close".to_string(),
            "feedback-working-close",
            false,
            || true,
            move || close(review, true),
        ),
    ))
    .style(|s| s.items_center().gap(8.0).width_full());
    frame(
        app,
        vec![
            header(
                || "Getting your report ready".to_string(),
                || "Nothing has left this Mac yet.".to_string(),
                Empty::new(),
            )
            .into_any(),
            steps.into_any(),
            hint.into_any(),
        ],
        footer,
        "feedback-working",
    )
}

/// The issue as it will be sent, and the ways to send it.
fn preview(app: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let start =
        draft_with(app, Clone::clone).unwrap_or_else(|| FeedbackDraft::new(FeedbackKind::Bug));
    let sending = move || draft_with(app, |d| d.pending.is_some()).unwrap_or(false);

    let mut notes: Vec<AnyView> = Vec::new();
    if start.caught_on_send > 0 {
        let n = start.caught_on_send;
        notes.push(
            note(
                theme::draft_yellow,
                move || {
                    format!(
                        "Redacted {} you added. Check it and file again.",
                        count_of(n, "value", "values")
                    )
                },
                "feedback-caught",
            )
            .into_any(),
        );
    } else if start.redacted > 0 {
        let n = start.redacted;
        notes.push(
            note(
                theme::open,
                move || {
                    format!(
                        "Redacted {}: tokens, emails, your user name and other repositories.",
                        count_of(n, "value", "values")
                    )
                },
                "feedback-redacted",
            )
            .into_any(),
        );
    }
    if let Some(why) = start.notes_error.clone() {
        notes.push(
            note(
                theme::fg_subtle,
                move || format!("No agent notes. {why}"),
                "feedback-notes-error",
            )
            .into_any(),
        );
    }

    let body = text_box(
        review,
        start.preview.clone(),
        String::new(),
        (220.0, 300.0),
        "feedback-preview-body",
        sending,
        move |text| {
            if draft_with(app, |d| d.preview != text).unwrap_or(false) {
                update(app, |d| d.preview = text);
            }
        },
    );

    let similar = similar_issues(
        app,
        review,
        backend.clone(),
        start.similar.clone(),
        start.kind,
    );

    let error = dyn_container(
        move || draft_with(app, |d| d.error.clone()).flatten(),
        move |error| -> AnyView {
            let Some(error) = error else {
                return Empty::new().into_any();
            };
            let forbidden = error.forbidden;
            Stack::horizontal((
                sidebar_glyph(SidebarIcon::Warning, theme::closed)
                    .style(|s| s.size(14.0, 14.0).margin_top(1.0).flex_shrink(0.0_f32)),
                Label::derived(move || error.message.clone())
                    .debug_name("feedback-error")
                    .style(|s| {
                        s.min_width(0.0)
                            .flex_grow(1.0_f32)
                            .flex_basis(0.0)
                            .font_size(theme::TEXT_META)
                            .color(theme::closed())
                            .text_wrap()
                    }),
                action_button(
                    || "Open on GitHub".to_string(),
                    "feedback-open-on-github",
                    false,
                    || true,
                    move || open_on_github(app),
                )
                .style(move |s| s.apply_if(!forbidden, |s| s.hide())),
            ))
            .style(|s| s.width_full().items_center().gap(7.0).margin_bottom(10.0))
            .into_any()
        },
    )
    .style(|s| s.width_full());

    let file_backend = backend.clone();
    let file = move || {
        let Some(draft) = app.feedback.get_untracked() else {
            return;
        };
        if draft.pending.is_some() || draft.title.trim().is_empty() {
            return;
        }
        update(app, |d| {
            d.pending = Some(Pending::File);
            d.step = Step::Filing;
            d.error = None;
            d.caught_on_send = 0;
        });
        file_backend.file_feedback(draft.title.trim(), &draft.preview, draft.kind, None);
    };
    let can_file = move || {
        draft_with(app, |d| {
            d.pending.is_none() && !d.title.trim().is_empty() && !d.preview.trim().is_empty()
        })
        .unwrap_or(false)
    };
    let footer = Stack::horizontal((
        action_button(
            || "Back".to_string(),
            "feedback-preview-back",
            false,
            move || !sending(),
            move || update(app, FeedbackDraft::back_to_form),
        ),
        Label::derived(move || format!("Posts to {REPO} as you.")).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .selectable(false)
        }),
        action_button(
            move || match draft_with(app, |d| d.pending).flatten() {
                Some(Pending::File) => "Filing…".to_string(),
                _ => "File issue".to_string(),
            },
            "feedback-file",
            true,
            can_file,
            file,
        ),
    ))
    .style(|s| s.items_center().gap(8.0).width_full());
    // A failure shows over the buttons, where it can't be scrolled away.
    let footer = Stack::vertical((error, footer)).style(|s| s.flex_col().width_full());

    let title = header(
        || "Review your report".to_string(),
        || format!("This is exactly what goes to {REPO}. Edit anything."),
        Empty::new(),
    );
    let mut parts: Vec<AnyView> = vec![title.into_any()];
    parts.extend(notes);
    parts.push(title_input(app, "Title", sending).into_any());
    parts.push(body.into_any());
    parts.push(similar.into_any());
    frame(app, parts, footer, "feedback-preview-page")
}

/// The token can't open issues on Perch's repository: copy the body and
/// open GitHub's new-issue page with the title alone in its address.
fn open_on_github(app: AppState) {
    let Some(draft) = app.feedback.get_untracked() else {
        return;
    };
    crate::ui::widgets::copy_to_clipboard(draft.preview.clone());
    let title = percent_encode(draft.title.trim());
    crate::ui::widgets::open_in_browser(&format!(
        "https://github.com/{REPO}/issues/new?title={title}"
    ));
    app.notify(
        "The report is on your clipboard: paste it into GitHub's form",
        Tone::Success,
    );
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Already reported? The issues that look like this one, each with +1
/// and Comment there instead.
fn similar_issues(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    similar: Vec<SimilarIssue>,
    kind: FeedbackKind,
) -> impl IntoView {
    if similar.is_empty() {
        return Empty::new().into_any();
    }
    let mut rows: Vec<AnyView> = vec![Label::derived(|| "ALREADY REPORTED?".to_string())
        .style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_subtle())
                .selectable(false)
        })
        .into_any()];
    for (i, issue) in similar.into_iter().enumerate() {
        rows.push(similar_row(app, review, backend.clone(), issue, kind, i > 0).into_any());
    }
    Stack::vertical_from_iter(rows)
        .style(|s| s.flex_col().gap(8.0).width_full())
        .debug_name("feedback-similar")
        .into_any()
}

fn similar_row(
    app: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    issue: SimilarIssue,
    kind: FeedbackKind,
    divided: bool,
) -> impl IntoView {
    let number = issue.number;
    let next = issue.plus_one();
    let node_id = issue.node_id.clone();
    let pending = move || draft_with(app, |d| d.pending).flatten();
    let idle = move || pending().is_none();

    let open_backend = backend.clone();
    let title = Stack::horizontal((
        Label::derived(move || format!("#{number}")).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
        Label::derived({
            let title = issue.title.clone();
            move || title.clone()
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .selectable(false)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| {
        // The draft stays; the bug icon brings it back.
        close(review, false);
        let navigation_id = open_backend.open_subject(REPO, perch_github::ItemKind::Issue, number);
        app.begin_open("Reading the issue", navigation_id);
    })
    .style(|s| {
        clickable(s)
            .items_center()
            .gap(6.0)
            .width_full()
            .hover(|s| s.color(theme::accent()))
    })
    .debug_name(format!("feedback-similar-open-{number}"));

    let updated = issue
        .updated_at
        .map(|at| format!(" · {}", crate::timefmt::relative(at, Timestamp::now())))
        .unwrap_or_default();
    let meta = format!("👍 {}{updated}", issue.thumbs_up);

    let plus_backend = backend.clone();
    let plus = action_button(
        move || match (pending(), next) {
            (Some(Pending::PlusOne(n)), _) if n == number => "Sending…".to_string(),
            (_, PlusOne::React) => "+1".to_string(),
            (_, PlusOne::Subscribe) => "Subscribe".to_string(),
            (_, PlusOne::Done) => String::new(),
        },
        "feedback-plus-one",
        false,
        idle,
        move || {
            update(app, |d| {
                d.pending = Some(Pending::PlusOne(number));
                d.error = None;
            });
            plus_backend.plus_one_feedback(number, &node_id, next == PlusOne::React);
        },
    );
    let plus: AnyView = if next == PlusOne::Done {
        Label::derived(|| "You +1'd this".to_string())
            .style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .selectable(false)
                    .flex_shrink(0.0_f32)
            })
            .into_any()
    } else {
        plus.into_any()
    };

    let comment_backend = backend.clone();
    let comment = action_button(
        move || match pending() {
            Some(Pending::Comment(n)) if n == number => "Commenting…".to_string(),
            _ => "Comment there instead".to_string(),
        },
        "feedback-comment",
        false,
        move || idle() && draft_with(app, |d| !d.preview.trim().is_empty()).unwrap_or(false),
        move || {
            let Some(draft) = app.feedback.get_untracked() else {
                return;
            };
            update(app, |d| {
                d.pending = Some(Pending::Comment(number));
                d.step = Step::Filing;
                d.error = None;
                d.caught_on_send = 0;
            });
            comment_backend.file_feedback(
                draft.title.trim(),
                &draft.preview,
                draft.kind,
                Some(number),
            );
        },
    );

    Stack::vertical((
        title,
        Stack::horizontal((
            Label::derived(move || meta.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .flex_basis(0.0)
                    .text_ellipsis()
                    .selectable(false)
            }),
            // The one that suits the kind comes first: a +1 says all an
            // idea needs; a bug's second report is worth its diagnostics.
            if kind == FeedbackKind::Idea {
                Stack::horizontal((plus, comment.into_any()))
            } else {
                Stack::horizontal((comment.into_any(), plus))
            }
            .style(|s| s.items_center().gap(6.0).flex_shrink(0.0_f32)),
        ))
        .style(|s| s.items_center().gap(8.0).width_full()),
    ))
    .style(move |s| {
        s.flex_col()
            .gap(6.0)
            .width_full()
            .padding(10.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .apply_if(divided, |s| s)
    })
    .debug_name(format!("feedback-similar-{number}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use floem::headless::{HeadlessHarness, TestRoot};

    use crate::backend::Command;
    use crate::feedback::{Notes, Prepared, SendError, Sent, Subscription};
    use crate::review::header::tests::{named, pull_request_harness, rect};

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
    }

    /// The window with nothing open, its commands recorded, and both
    /// CLIs installed.
    fn empty_window() -> (
        HeadlessHarness,
        AppState,
        ReviewState,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let app = AppState::new();
        let review = ReviewState::new();
        app.review.set(Some(review));
        app.agent.update(|agent| {
            agent.models = vec!["opus".into()];
            agent.codex.models = vec!["gpt-6".into()];
        });
        let (backend, commands) = Backend::recording();
        let root = TestRoot::new();
        let view = crate::ui::app_view(app, review, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        settle(&mut harness);
        (harness, app, review, commands)
    }

    fn click(harness: &mut HeadlessHarness, name: &str) {
        let at = rect(harness, name).center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    fn sent(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| commands.try_recv().ok()).collect()
    }

    fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        named(harness.root_id(), name).is_some_and(|id| {
            let r = id.get_visual_rect_no_clip();
            r.width() > 0.0 && r.height() > 0.0
        })
    }

    fn problem(text: &str) -> Problem {
        Problem {
            at: Timestamp::UNIX_EPOCH,
            text: text.into(),
            tone: Tone::Error,
        }
    }

    /// A draft on its preview, as `FeedbackReady` leaves it.
    fn previewing(app: AppState, similar: Vec<SimilarIssue>) {
        let mut draft = FeedbackDraft::from_problem(problem("rust-analyzer failed"));
        draft.preview = "It hung.\n\n---\n\nPerch 0.1.0".into();
        draft.step = Step::Preview;
        draft.similar = similar;
        draft.generation = 1;
        app.feedback.set(Some(draft));
    }

    fn similar(number: u64, reacted: bool, subscription: Subscription) -> SimilarIssue {
        SimilarIssue {
            number,
            title: format!("Issue {number}"),
            updated_at: None,
            node_id: format!("I_{number}"),
            thumbs_up: 2,
            viewer_thumbs_up: reacted,
            viewer_subscription: subscription,
        }
    }

    #[test]
    fn the_bug_icon_sits_just_before_usage_in_the_drawers_footer() {
        let (harness, _app, _review, _commands) = empty_window();
        let footer = rect(&harness, "drawer footer");
        assert!((footer.height() - theme::TITLEBAR_HEIGHT).abs() < 1.0, "{footer:?}");
        let bug = rect(&harness, "feedback-button");
        let gauge = rect(&harness, "usage-button");
        assert!(bug.width() > 0.0, "with nothing open");
        assert!(
            bug.x1 <= gauge.x0 && gauge.x0 - bug.x1 < 16.0,
            "{bug:?} {gauge:?}"
        );
        assert!(footer.contains(bug.center()) && footer.contains(gauge.center()));
        assert!((footer.x1 - theme::NAV_GUTTER - gauge.x1).abs() < 1.0, "on the gutter: {footer:?} {gauge:?}");

        let (harness, _app, _review) = pull_request_harness();
        let footer = rect(&harness, "drawer footer");
        assert!(footer.contains(rect(&harness, "feedback-button").center()), "with an item open");
        let bar = named(harness.root_id(), "item bar").expect("an item's bar");
        assert!(named(bar, "feedback-button").is_none(), "gone from the item's bar");
        assert!(named(bar, "usage-button").is_none(), "gone from the item's bar");
    }

    #[test]
    fn the_icon_raises_a_bug_report_over_itself_and_folds_it_again() {
        let (mut harness, app, review, _commands) = empty_window();
        click(&mut harness, "feedback-button");
        assert!(review.feedback_open.get());
        assert_eq!(draft_with(app, |d| d.kind), Some(FeedbackKind::Bug));
        let button = rect(&harness, "feedback-button");
        let dropdown = rect(&harness, "feedback-dropdown");
        assert!(dropdown.y1 <= button.y0, "{dropdown:?} {button:?}");
        assert!((dropdown.x0 - button.x0).abs() < 1.0, "left edges aligned: {dropdown:?} {button:?}");
        assert!((dropdown.width() - theme::FOOTER_POPOVER_WIDTH).abs() < 1.0);
        assert!(dropdown.height() <= theme::FEEDBACK_DROPDOWN_MAX_HEIGHT + 1.0);
        assert!(shown(&harness, "feedback-form"));
        click(&mut harness, "feedback-button");
        assert!(!review.feedback_open.get());
        assert!(
            app.feedback.with(Option::is_some),
            "folding keeps the draft"
        );
    }

    /// The title's caret and the description's draw in the text colour,
    /// light or dark. floem's own was 30% black, gone on a dark window
    /// (criccomini/perch#8).
    #[test]
    fn the_carets_show_in_the_dark() {
        use floem::peniko::Brush;
        use floem::style::{CursorColor, StyleProp};

        fn carets(harness: &HeadlessHarness, id: floem::ViewId, out: &mut Vec<Brush>) {
            let caret = harness.get_computed_style(id).get(CursorColor);
            if caret != CursorColor::default_value() {
                out.push(caret);
            }
            for child in id.children() {
                carets(harness, child, out);
            }
        }

        let (mut harness, app, review, _commands) = empty_window();
        open(app, review, FeedbackKind::Bug, None);
        settle(&mut harness);
        crate::theme::set_appearance(crate::theme::Appearance::Dark);
        settle(&mut harness);
        let fg = Brush::Solid(theme::fg());
        let found = ["feedback-title", "feedback-body"].map(|name| {
            let mut found = Vec::new();
            carets(&harness, named(harness.root_id(), name).unwrap(), &mut found);
            (name, found)
        });
        crate::theme::set_appearance(crate::theme::Appearance::Light);
        for (name, found) in found {
            assert_eq!(found, std::slice::from_ref(&fg), "{name}");
        }
    }

    #[test]
    fn send_feedback_opens_an_idea_unless_a_draft_is_there() {
        let (mut harness, app, review, _commands) = empty_window();
        open(app, review, FeedbackKind::Idea, None);
        settle(&mut harness);
        assert_eq!(
            draft_with(app, |d| (d.kind, d.diagnose)),
            Some((FeedbackKind::Idea, false))
        );
        close(review, false);
        update(app, |d| d.set_kind(FeedbackKind::Bug));
        open(app, review, FeedbackKind::Idea, None);
        assert_eq!(
            draft_with(app, |d| d.kind),
            Some(FeedbackKind::Bug),
            "the draft stays"
        );
    }

    #[test]
    fn report_on_an_error_toast_starts_a_bug_about_it() {
        let (mut harness, app, review, _commands) = empty_window();
        app.complain("rust-analyzer failed: the language server did not answer in time");
        settle(&mut harness);
        click(&mut harness, "Report problem");
        assert!(app.current_toast().is_none(), "the toast goes");
        assert!(review.feedback_open.get());
        let draft = app.feedback.get().unwrap();
        assert_eq!(draft.kind, FeedbackKind::Bug);
        assert_eq!(
            draft.title,
            "rust-analyzer failed: the language server did not answer in time"
        );
        assert_eq!(
            draft.seed.map(|s| s.text).as_deref(),
            Some("rust-analyzer failed: the language server did not answer in time")
        );
        assert!(shown(&harness, "feedback-seed"));
    }

    #[test]
    fn a_success_toast_has_no_report() {
        let (mut harness, app, _review, _commands) = empty_window();
        app.notify("Merged a/b#1", Tone::Success);
        settle(&mut harness);
        assert!(!shown(&harness, "Report problem"));
    }

    #[test]
    fn a_second_report_asks_before_replacing_the_draft() {
        let (mut harness, app, review, _commands) = empty_window();
        report(app, review, "first".into(), Tone::Error);
        update(app, |d| d.body = "What I saw.".into());
        report(app, review, "second".into(), Tone::Warning);
        settle(&mut harness);
        assert_eq!(
            draft_with(app, |d| d.title.clone()).as_deref(),
            Some("first")
        );
        assert!(shown(&harness, "feedback-offer-replace"));
        click(&mut harness, "feedback-offer-replace");
        assert_eq!(
            draft_with(app, |d| d.title.clone()).as_deref(),
            Some("second")
        );
    }

    #[test]
    fn the_dropdowns_shut_each_other_and_the_draft_waits() {
        let (mut harness, app, review, _commands) = empty_window();
        click(&mut harness, "feedback-button");
        update(app, |d| d.title = "Half a report".into());
        click(&mut harness, "agents-button");
        assert!(review.agents_dropdown_open.get());
        assert!(!review.feedback_open.get());
        click(&mut harness, "feedback-button");
        assert!(review.feedback_open.get());
        assert!(!review.agents_dropdown_open.get());
        assert_eq!(
            draft_with(app, |d| d.title.clone()).as_deref(),
            Some("Half a report")
        );

        // A press outside shuts it too.
        let page = rect(&harness, "main-pane");
        harness.click(page.x0 + 600.0, page.y0 + 600.0);
        settle(&mut harness);
        assert!(!review.feedback_open.get());
        assert!(app.feedback.with(Option::is_some));
    }

    #[test]
    fn preview_hands_the_backend_the_draft_and_shows_the_steps() {
        let (mut harness, app, _review, mut commands) = empty_window();
        app.complain("rust-analyzer failed");
        click(&mut harness, "feedback-button");
        update(app, |d| {
            d.title = "RA failed".into();
            d.body = "It hung.".into();
        });
        settle(&mut harness);
        click(&mut harness, "feedback-preview");
        let sent = sent(&mut commands);
        let request = sent
            .iter()
            .find_map(|c| match c {
                Command::PrepareFeedback(request) => Some(request.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no PrepareFeedback: {sent:?}"));
        assert_eq!(request.title, "RA failed");
        assert_eq!(request.body, "It hung.");
        assert!(request.diagnose);
        assert_eq!(
            request.choice.as_ref().map(|c| c.0),
            Some(perch_agent::Backend::ClaudeCode)
        );
        assert_eq!(request.problems.len(), 1, "the session's problems go too");
        assert_eq!(request.attempt, draft_with(app, |d| d.attempt).unwrap());
        assert!(shown(&harness, "feedback-working"));
    }

    #[test]
    fn preview_needs_a_title() {
        let (mut harness, _app, _review, mut commands) = empty_window();
        click(&mut harness, "feedback-button");
        click(&mut harness, "feedback-preview");
        assert!(!sent(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::PrepareFeedback(_))));
    }

    fn ready(attempt: u64) -> Event {
        Event::FeedbackReady(Box::new(Prepared {
            attempt,
            context: Default::default(),
            notes: Some(Notes {
                by: "Claude".into(),
                text: "### Likely cause".into(),
            }),
            notes_error: None,
            preview: "The issue as it goes.".into(),
            redacted: 2,
            similar: Vec::new(),
        }))
    }

    #[test]
    fn an_answer_to_an_old_preview_is_dropped() {
        let (mut harness, app, review, _commands) = empty_window();
        let mut draft = FeedbackDraft::new(FeedbackKind::Bug);
        draft.title = "x".into();
        draft.step = Step::Diagnosing;
        draft.attempt = 3;
        app.feedback.set(Some(draft));
        review.feedback_open.set(true);
        apply(app, review, ready(2));
        assert_eq!(draft_with(app, |d| d.step), Some(Step::Diagnosing));
        apply(app, review, ready(3));
        settle(&mut harness);
        assert_eq!(draft_with(app, |d| d.step), Some(Step::Preview));
        assert_eq!(
            draft_with(app, |d| d.preview.clone()).as_deref(),
            Some("The issue as it goes.")
        );
        assert!(shown(&harness, "feedback-redacted"));
        assert!(app.current_toast().is_none(), "no toast while it's showing");
    }

    #[test]
    fn a_report_ready_while_shut_says_so_and_the_toast_opens_it() {
        let (mut harness, app, review, _commands) = empty_window();
        let mut draft = FeedbackDraft::new(FeedbackKind::Bug);
        draft.step = Step::Diagnosing;
        app.feedback.set(Some(draft));
        apply(app, review, ready(0));
        settle(&mut harness);
        let toast = app.current_toast().unwrap();
        assert!(matches!(
            toast.kind,
            crate::state::ToastKind::Message { ref text, open: Some(ToastOpen::FeedbackPreview), .. }
                if text == "Your report is ready to review"
        ));
        click(&mut harness, "toast");
        assert!(review.feedback_open.get());
        assert!(shown(&harness, "feedback-preview-page"));
    }

    #[test]
    fn file_issue_sends_the_preview_as_it_stands() {
        let (mut harness, app, review, mut commands) = empty_window();
        previewing(app, Vec::new());
        review.feedback_open.set(true);
        settle(&mut harness);
        update(app, |d| d.preview.push_str("\n\nEdited."));
        settle(&mut harness);
        click(&mut harness, "feedback-file");
        let sent = sent(&mut commands);
        assert!(
            sent.iter().any(|c| matches!(
                c,
                Command::FileFeedback { title, body, comment_on: None, .. }
                    if title == "rust-analyzer failed" && body == "It hung.\n\n---\n\nPerch 0.1.0\n\nEdited."
            )),
            "{sent:?}"
        );
        assert_eq!(draft_with(app, |d| d.pending), Some(Some(Pending::File)));
    }

    #[test]
    fn a_sent_report_leaves_the_window_as_it_was_and_toasts_the_issue() {
        let (mut harness, app, review) = pull_request_harness();
        previewing(app, Vec::new());
        review.feedback_open.set(true);
        settle(&mut harness);
        let item = review.item.with(|i| i.as_ref().map(|i| i.id.clone()));
        apply(
            app,
            review,
            Event::FeedbackSent {
                number: 123,
                outcome: Sent::Filed,
            },
        );
        settle(&mut harness);
        assert!(app.feedback.with(Option::is_none));
        assert!(!review.feedback_open.get());
        assert_eq!(
            review.item.with(|i| i.as_ref().map(|i| i.id.clone())),
            item,
            "still on the pull request"
        );
        let toast = app.current_toast().unwrap();
        assert!(matches!(
            toast.kind,
            crate::state::ToastKind::Message {
                ref text,
                open: Some(ToastOpen::Subject { number: 123, .. }),
                ..
            } if text == "Filed perch-code/perch#123"
        ));
    }

    #[test]
    fn a_caught_secret_puts_the_redacted_text_back_to_check() {
        let (mut harness, app, review, _commands) = empty_window();
        previewing(app, Vec::new());
        review.feedback_open.set(true);
        update(app, |d| d.pending = Some(Pending::File));
        apply(
            app,
            review,
            Event::FeedbackCaught {
                title: "rust-analyzer failed".into(),
                preview: "my token is ‹redacted›".into(),
                count: 1,
            },
        );
        settle(&mut harness);
        assert_eq!(
            draft_with(app, |d| d.preview.clone()).as_deref(),
            Some("my token is ‹redacted›")
        );
        assert_eq!(draft_with(app, |d| d.pending), Some(None));
        assert!(shown(&harness, "feedback-caught"));
    }

    #[test]
    fn a_token_that_cannot_file_offers_github() {
        let (mut harness, app, review, _commands) = empty_window();
        previewing(app, Vec::new());
        review.feedback_open.set(true);
        apply(
            app,
            review,
            Event::FeedbackFailed(SendError {
                message: "Your token can't open issues on perch-code/perch.".into(),
                forbidden: true,
            }),
        );
        settle(&mut harness);
        assert!(shown(&harness, "feedback-error"));
        click(&mut harness, "feedback-open-on-github");
        assert_eq!(
            crate::ui::widgets::copied(),
            vec!["It hung.\n\n---\n\nPerch 0.1.0".to_string()],
            "the body goes on the clipboard"
        );
        let opened = crate::ui::widgets::opened();
        assert_eq!(
            opened,
            vec![
                "https://github.com/perch-code/perch/issues/new?title=rust-analyzer+failed"
                    .to_string()
            ]
        );
    }

    #[test]
    fn similar_issues_offer_plus_one_subscribe_or_nothing() {
        let (mut harness, app, review, mut commands) = empty_window();
        previewing(
            app,
            vec![
                similar(41, false, Subscription::Unsubscribed),
                similar(37, true, Subscription::Unsubscribed),
                similar(12, true, Subscription::Subscribed),
            ],
        );
        review.feedback_open.set(true);
        settle(&mut harness);
        let root = harness.root_id();
        let row = move |n: u64| named(root, &format!("feedback-similar-{n}")).unwrap();
        assert!(named(row(41), "\"+1\"").is_some());
        assert!(named(row(37), "\"Subscribe\"").is_some());
        assert!(named(row(12), "You +1'd this").is_some());

        let plus = named(row(41), "feedback-plus-one")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
        harness.click(plus.x, plus.y);
        settle(&mut harness);
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::PlusOneFeedback { number: 41, react: true, node_id } if node_id == "I_41"
        )));
        update(app, |d| d.pending = None);
        settle(&mut harness);
        let subscribe = named(row(37), "feedback-plus-one")
            .unwrap()
            .get_visual_rect_no_clip()
            .center();
        harness.click(subscribe.x, subscribe.y);
        settle(&mut harness);
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::PlusOneFeedback {
                number: 37,
                react: false,
                ..
            }
        )));
    }

    #[test]
    fn comment_there_instead_sends_the_preview_to_that_issue() {
        let (mut harness, app, review, mut commands) = empty_window();
        previewing(app, vec![similar(41, false, Subscription::Unsubscribed)]);
        review.feedback_open.set(true);
        settle(&mut harness);
        click(&mut harness, "feedback-comment");
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::FileFeedback { comment_on: Some(41), body, .. } if body == "It hung.\n\n---\n\nPerch 0.1.0"
        )));
    }

    #[test]
    fn back_from_the_preview_returns_to_the_form_and_drops_what_was_made() {
        let (mut harness, app, review, _commands) = empty_window();
        previewing(app, vec![similar(41, false, Subscription::Unsubscribed)]);
        review.feedback_open.set(true);
        settle(&mut harness);
        let before = draft_with(app, |d| d.attempt).unwrap();
        click(&mut harness, "feedback-preview-back");
        assert!(shown(&harness, "feedback-form"));
        assert_eq!(draft_with(app, |d| d.step), Some(Step::Editing));
        assert!(draft_with(app, |d| d.similar.is_empty()).unwrap());
        assert_eq!(
            draft_with(app, |d| d.attempt),
            Some(before + 1),
            "an old answer can't land"
        );
    }

    #[test]
    fn the_box_names_codex_when_codex_is_picked() {
        let (mut harness, app, review, _commands) = empty_window();
        review.agent_last.set(Some((
            perch_agent::Backend::Codex,
            "gpt-6".into(),
            Default::default(),
        )));
        click(&mut harness, "feedback-button");
        assert!(named(harness.root_id(), "Let Codex look into it").is_some());
        app.agent.update(|agent| agent.codex.models.clear());
        settle(&mut harness);
        assert!(
            named(harness.root_id(), "Let Claude look into it").is_some(),
            "the CLI there is"
        );
    }
}
