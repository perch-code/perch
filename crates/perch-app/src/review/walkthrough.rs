//! Walkthrough: the change told start to finish, as an agent wrote it and
//! the backend's `walkthrough` checked it. One reading column, as
//! Conversation's: a card with the change's summary that writes it, then
//! the background and a card for each step, each its prose with the lines
//! it is about set in it, which Perch draws from the files as a thread's
//! card draws its lines, and the threads it names, which open on
//! Conversation. All of it is Perch's own, so all of it is grey.
//!
//! The lines are read at the commits the walkthrough was written for, not
//! taken from Code, so they are its lines whatever Code shows. One written
//! for an older head still shows them, as they were, and says so.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use perch_github::{Anchor, Author, CommentThread};

use crate::backend::{Backend, CodeSide, Part, Step, Walkthrough, WalkthroughOutcome};
use crate::review::cards::{self, perch_surface};
use crate::review::doc::CardId;
use crate::review::model::{AgentChoice, ItemTab, LoadedFile, Paging, ReviewState};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{action_button, clickable};

/// The most lines a block shows before it says how many more.
const BLOCK_LINES: usize = 40;

/// Fold the backend's answer into the item's walkthrough.
pub fn apply(review: ReviewState, item_id: String, outcome: WalkthroughOutcome) {
    let done = |review: ReviewState, item_id: &str| {
        review.walkthroughs_writing.update(|writing| {
            writing.remove(item_id);
        });
        review.walkthrough_steps.update(|steps| {
            steps.remove(item_id);
        });
    };
    // The guidance a kept walkthrough was written with fills the box,
    // unless the reader has written there already.
    let remember = |review: ReviewState, item_id: &str, guidance: &str| {
        if !guidance.is_empty() {
            review.walkthrough_guidance.update(|drafts| {
                drafts
                    .entry(item_id.to_string())
                    .or_insert_with(|| guidance.to_string());
            });
        }
    };
    match outcome {
        WalkthroughOutcome::Loaded(kept) => {
            if let Some(kept) = &kept {
                remember(review, &item_id, &kept.guidance);
            }
            review.walkthroughs.update(|walkthroughs| {
                walkthroughs.insert(item_id, kept.map(Arc::new));
            })
        }
        WalkthroughOutcome::Written(walkthrough) => {
            done(review, &item_id);
            remember(review, &item_id, &walkthrough.guidance);
            review.walkthrough_errors.update(|errors| {
                errors.remove(&item_id);
            });
            review.walkthroughs.update(|walkthroughs| {
                walkthroughs.insert(item_id, Some(Arc::new(walkthrough)));
            });
        }
        WalkthroughOutcome::Failed(message) => {
            done(review, &item_id);
            review.walkthrough_errors.update(|errors| {
                errors.insert(item_id, message);
            });
        }
        WalkthroughOutcome::Stopped => done(review, &item_id),
    }
}

/// What the agent writing the item's walkthrough has done so far. Steps
/// that come after it is done are of a run already over.
pub fn progress(review: ReviewState, item_id: String, steps: Vec<perch_github::RunStep>) {
    if review
        .walkthroughs_writing
        .with_untracked(|writing| writing.contains_key(&item_id))
    {
        review.walkthrough_steps.update(|all| {
            all.insert(item_id, steps);
        });
    }
}

/// The agent a walkthrough is written with: the one picked here, else
/// the one last asked anywhere, else config's.
fn choice(app: AppState, state: ReviewState) -> AgentChoice {
    state
        .walkthrough_agent
        .get()
        .or_else(|| state.agent_last.get())
        .unwrap_or_else(|| crate::review::threads::defaults(app))
}

/// Ask for a walkthrough of the open item, with the agent picked and the
/// reader's guidance.
fn write(app: AppState, state: ReviewState, backend: &Backend) {
    let Some(item_id) = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
    else {
        return;
    };
    let choice = floem::reactive::Effect::untrack(|| choice(app, state));
    let guidance = state
        .walkthrough_guidance
        .with_untracked(|drafts| drafts.get(&item_id).cloned())
        .unwrap_or_default();
    state.agent_last.set(Some(choice.clone()));
    state.walkthroughs_writing.update(|writing| {
        writing.insert(item_id.clone(), jiff::Timestamp::now());
    });
    state.walkthrough_steps.update(|steps| {
        steps.remove(&item_id);
    });
    state.walkthrough_errors.update(|errors| {
        errors.remove(&item_id);
    });
    backend.write_walkthrough(&item_id, choice, &guidance);
}

pub fn view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    let item = Memo::new(move |_| state.item.with(|i| i.as_ref().map(|i| i.id.clone())));
    // What is kept for the item, asked for the first time it shows here.
    {
        let backend = backend.clone();
        floem::reactive::Effect::new(move |_| {
            let Some(id) = item.get() else {
                return;
            };
            if !state
                .walkthroughs
                .with_untracked(|kept| kept.contains_key(&id))
            {
                backend.load_walkthrough(&id);
            }
        });
    }
    let shown = Memo::new(move |_| {
        let id = item.get()?;
        state
            .walkthroughs
            .with(|kept| kept.get(&id).cloned().flatten())
    });
    // Whether it was written for a head other than the checkout's.
    let stale = Memo::new(move |_| {
        let head = state.item.with(|i| {
            i.as_ref()
                .and_then(|i| i.checkout.as_ref())
                .map(|c| c.head_sha.clone())
        });
        shown.with(|w| w.as_ref().zip(head).is_some_and(|(w, head)| w.head != head))
    });

    let page_id: RwSignal<Option<floem::ViewId>> = RwSignal::new(None);
    let steps_backend = backend.clone();
    let page = Container::new(
        Stack::vertical((
            Label::derived(|| "Walkthrough".to_string()).style(|s| {
                s.width_full()
                    .font_size(theme::TEXT_PAGE_TITLE)
                    .font_weight(theme::weight_emphasis())
                    .line_height(theme::PAGE_TITLE_LINE_HEIGHT)
                    .color(theme::fg())
                    .padding_bottom(theme::CONVERSATION_TITLE_GAP)
            }),
            head_card(app, state, backend, item, shown, stale),
            dyn_container(
                move || shown.get(),
                move |shown| match (shown, item.get_untracked()) {
                    (Some(walkthrough), Some(item_id)) => steps(
                        state,
                        &steps_backend,
                        open_file.clone(),
                        item_id,
                        walkthrough,
                    )
                    .into_any(),
                    _ => Empty::new().into_any(),
                },
            )
            .style(|s| s.width_full()),
        ))
        // A definite width cut to the page's, as Conversation's column
        // has, so wrapped prose is measured at the width it is drawn at.
        .style(|s| {
            s.flex_col()
                .width(theme::CONVERSATION_WIDTH)
                .max_width_full()
        }),
    )
    .style(|s| {
        s.width_full()
            .items_start()
            .justify_center()
            .padding(theme::CONVERSATION_PADDING)
    })
    .scroll()
    // Page Up and Page Down, and Cmd-Up and Cmd-Down, as on Conversation.
    .scroll_delta(move || {
        let Some(paging) = state.paging.get() else {
            return floem::kurbo::Vec2::ZERO;
        };
        state.paging.set(None);
        let page = || {
            let height = page_id
                .get_untracked()
                .map_or(0.0, |id| id.get_content_rect_local().height());
            (height - theme::code_line_height() as f64).max(0.0)
        };
        let y = match paging {
            Paging::Up => -page(),
            Paging::Down => page(),
            Paging::Top => -f64::MAX,
            Paging::End => f64::MAX,
        };
        floem::kurbo::Vec2::new(0.0, y)
    })
    .debug_name("walkthrough")
    .style(|s| {
        s.size_full()
            .min_height(0.0)
            .background(theme::canvas())
            .focusable()
    });
    page_id.set(Some(page.id()));
    // The page takes the keyboard when it comes up with nothing else
    // holding it, so Page Down reaches the window's handler.
    let asked = Rc::new(std::cell::Cell::new(false));
    let focus_id = page.id();
    Container::new(page)
        .on_event_cont(el::UpdatePhaseLayout, move |cx, _| {
            if !asked.replace(true) && crate::ui::focused_descendant(cx, focus_id.root()).is_none()
            {
                focus_id.request_focus();
            }
        })
        .style(|s| s.size_full().min_height(0.0))
}

/// The card over the steps: the change's summary once there is one, and
/// under it what a walkthrough is before there is one, that it is being
/// written, why it could not be, or that it is of an older head. Under
/// that the reader's guidance, the model and effort, and Write, or Write
/// again, or Stop.
fn head_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    item: Memo<Option<String>>,
    shown: Memo<Option<Arc<Walkthrough>>>,
    stale: Memo<bool>,
) -> impl IntoView {
    let started = Memo::new(move |_| {
        let id = item.get()?;
        state
            .walkthroughs_writing
            .with(|writing| writing.get(&id).copied())
    });
    let writing = Memo::new(move |_| started.get().is_some());
    let error = Memo::new(move |_| {
        let id = item.get()?;
        state
            .walkthrough_errors
            .with(|errors| errors.get(&id).cloned())
    });
    let words = move || {
        if writing.get() {
            return String::new();
        }
        if let Some(error) = error.get() {
            return format!("The walkthrough couldn't be written: {error}");
        }
        match shown.get() {
            None => "An agent reads the change and tells it start to finish, in the order \
                     that's easiest to follow. Perch draws the code under each step, and \
                     checks that every hunk is in one."
                .to_string(),
            Some(w) if stale.get() => format!(
                "Written for {}, before the latest commits, and its code is as it was then. \
                 Write it again to see the change as it is.",
                crate::review::timeline::short_sha(&w.head)
            ),
            Some(_) => String::new(),
        }
    };
    // What the change is, over whatever is being said about the walkthrough.
    let summary = Memo::new(move |_| {
        shown
            .with(|w| w.as_ref().map(|w| w.summary.clone()))
            .filter(|summary| !summary.trim().is_empty())
    });
    let text = Stack::vertical((
        dyn_container(
            move || summary.get(),
            move |summary| match summary {
                Some(summary) => crate::review::markdown::render_linked(&summary, state).into_any(),
                None => Empty::new().into_any(),
            },
        )
        .debug_name("walkthrough-summary")
        .style(|s| s.width_full()),
        Label::derived(words).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .color(theme::fg_muted())
                .text_wrap()
                .width_full()
                .apply_if(words().is_empty(), |s| s.hide())
        }),
    ))
    .style(move |s| {
        s.flex_col()
            .gap(8.0)
            .width_full()
            .apply_if(summary.with(Option::is_none) && words().is_empty(), |s| {
                s.hide()
            })
    });
    let write_backend = backend.clone();
    let stop_backend = backend.clone();
    let controls = Stack::horizontal((
        agent_picker(app, state, writing),
        effort_picker(app, state, writing),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        action_button(
            move || {
                if shown.with(Option::is_some) {
                    "Write again".to_string()
                } else {
                    "Write".to_string()
                }
            },
            "walkthrough-write",
            false,
            || true,
            move || write(app, state, &write_backend),
        )
        .style(|s| s.flex_shrink(0.0_f32)),
    ))
    .style(move |s| {
        s.items_center()
            .width_full()
            .gap(6.0)
            .apply_if(writing.get(), |s| s.hide())
    });
    Stack::vertical((
        text,
        guidance_box(app, state, backend, item, writing),
        controls,
        writing_bar(state, item, started, stop_backend),
    ))
    .debug_name("walkthrough-head")
    .style(|s| {
        perch_surface(s)
            .flex_col()
            .width_full()
            .gap(12.0)
            .padding(theme::CARD_PADDING)
    })
}

/// While one is written: what the agent is doing, how long it has gone,
/// and Stop, over the steps it has taken, as a running thread has them.
fn writing_bar(
    state: ReviewState,
    item: Memo<Option<String>>,
    started: Memo<Option<jiff::Timestamp>>,
    backend: Rc<Backend>,
) -> impl IntoView {
    use crate::review::run_view::{doing_row, doing_step, timeline};

    let steps = Memo::new(move |_| {
        let id = item.get()?;
        state
            .walkthrough_steps
            .with(|steps| steps.get(&id).cloned())
    });
    let doing = Memo::new(move |_| {
        steps
            .with(|steps| steps.as_deref().and_then(doing_step))
            .unwrap_or_else(|| ("Writing the walkthrough".to_string(), String::new(), false))
    });
    let now = RwSignal::new(jiff::Timestamp::now());
    let elapsed = Label::derived(move || {
        let Some(started) = started.get() else {
            return String::new();
        };
        let ms = now.get().duration_since(started).as_millis().max(0);
        format!(
            "\u{00b7} {}",
            crate::review::threads::worked_for(u64::try_from(ms).unwrap_or(0))
        )
    })
    .debug_name("walkthrough-elapsed")
    .style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::fg_subtle())
            .margin_left(6.0)
            .selectable(false)
            .flex_shrink(0.0_f32)
    });
    // A clock of its own, ticking while one is written and the bar is up.
    let clock = elapsed.id();
    let ticking = Rc::new(std::cell::Cell::new(false));
    floem::reactive::Effect::new(move |_| {
        if started.get().is_some() && !ticking.replace(true) {
            now.set(jiff::Timestamp::now());
            tick(clock, now, started, ticking.clone());
        }
    });
    let stop = cards::tip(
        crate::ui::widgets::icon_button(
            || crate::icons::SidebarIcon::Stop,
            theme::fg_muted,
            move || {
                if let Some(id) = item.get_untracked() {
                    backend.stop_walkthrough(&id);
                }
            },
        )
        .debug_name("walkthrough-stop"),
        "Stop".to_string(),
    );
    let header = Stack::horizontal((
        doing_row(doing),
        elapsed,
        Empty::new().style(|s| s.flex_grow(1.0_f32).min_width(8.0)),
        Container::new(stop).style(|s| s.flex_shrink(0.0_f32)),
    ))
    .style(|s| s.items_center().width_full().height(28.0));
    Stack::vertical((
        header,
        timeline(state, move || steps.get().unwrap_or_default(), || true),
    ))
    .debug_name("walkthrough-writing")
    .style(move |s| {
        s.flex_col()
            .width_full()
            .gap(4.0)
            .apply_if(started.get().is_none(), |s| s.hide())
    })
}

fn tick(
    clock: floem::ViewId,
    now: RwSignal<jiff::Timestamp>,
    started: Memo<Option<jiff::Timestamp>>,
    ticking: Rc<std::cell::Cell<bool>>,
) {
    floem::action::exec_after(std::time::Duration::from_secs(1), move |_| {
        if clock.is_valid() && started.get_untracked().is_some() {
            now.set(jiff::Timestamp::now());
            tick(clock, now, started, ticking);
        } else {
            ticking.set(false);
        }
    });
}

/// The model to write with, Claude's then Codex's, as the composer lists
/// them. Hidden while one writes.
fn agent_picker(app: AppState, state: ReviewState, writing: Memo<bool>) -> impl IntoView {
    use crate::ui::menu::Menu;

    crate::review::review_launch::menu_button(
        move || {
            let (agent, model, _) = choice(app, state);
            crate::review::threads::model_name(agent.name(), &model)
        },
        move || {
            let (_, selected, effort) = floem::reactive::Effect::untrack(|| choice(app, state));
            let agent = app.agent.get_untracked();
            let mut menu = Menu::new();
            for (i, group) in crate::review::threads::model_groups(&agent)
                .into_iter()
                .enumerate()
            {
                if i > 0 {
                    menu = menu.separator();
                }
                for (backend, model) in group {
                    let checked = model == selected;
                    let effort = crate::review::threads::nearest_effort(&agent, &model, effort);
                    menu = menu.item(crate::review::threads::model_label(&model), move |item| {
                        let model = model.clone();
                        item.checked(checked).action(move || {
                            state
                                .walkthrough_agent
                                .set(Some((backend, model.clone(), effort)))
                        })
                    });
                }
            }
            menu
        },
        "walkthrough-model".to_string(),
    )
    .style(move |s| s.apply_if(writing.get(), |s| s.hide()))
}

/// The effort to write with. One the model doesn't take is shown but
/// can't be picked.
fn effort_picker(app: AppState, state: ReviewState, writing: Memo<bool>) -> impl IntoView {
    use crate::ui::menu::Menu;

    crate::review::review_launch::menu_button(
        move || choice(app, state).2.label().to_string(),
        move || {
            let (backend, model, selected) =
                floem::reactive::Effect::untrack(|| choice(app, state));
            let agent = app.agent.get_untracked();
            let mut menu = Menu::new();
            for effort in perch_agent::Effort::ALL {
                let takes = agent.effort_supported(&model, effort.name());
                let model = model.clone();
                menu = menu.item(effort.label(), move |item| {
                    item.checked(effort == selected)
                        .enabled(takes)
                        .action(move || {
                            state
                                .walkthrough_agent
                                .set(Some((backend, model.clone(), effort)))
                        })
                });
            }
            menu
        },
        "walkthrough-effort".to_string(),
    )
    .style(move |s| s.apply_if(writing.get(), |s| s.hide()))
}

/// What the reader wants the agent to keep in mind, kept per item until
/// they change it. Built again for each item, and once more when its kept
/// walkthrough arrives with the guidance it was written with. Cmd-Enter
/// writes. Hidden while one writes.
fn guidance_box(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    item: Memo<Option<String>>,
    writing: Memo<bool>,
) -> impl IntoView {
    use floem::views::editor::command::CommandExecuted;

    let key = Memo::new(move |_| {
        let id = item.get()?;
        let loaded = state.walkthroughs.with(|kept| kept.contains_key(&id));
        Some((id, loaded))
    });
    dyn_container(
        move || key.get(),
        move |key| {
            let Some((id, _)) = key else {
                return Empty::new().into_any();
            };
            let backend = backend.clone();
            let text = state
                .walkthrough_guidance
                .with_untracked(|drafts| drafts.get(&id).cloned())
                .unwrap_or_default();
            let editor = crate::ui::widgets::prose_editor(
                text,
                "Guidance for the agent (optional)",
                Some(state.next_token()),
            )
            .pre_command(move |cmd| {
                if crate::review::threads::cmd_enter(&cmd) {
                    write(app, state, &backend);
                    return CommandExecuted::Yes;
                }
                CommandExecuted::No
            })
            .update(move |update| {
                if let Some(editor) = update.editor {
                    let text = editor.doc().text().to_string();
                    state.walkthrough_guidance.update(|drafts| {
                        drafts.insert(id.clone(), text);
                    });
                }
            })
            .style(|s| {
                s.size_full()
                    .font_family(theme::FONT_STACK.to_string())
                    .font_size(theme::TEXT_BODY)
            })
            .debug_name("walkthrough-guidance");
            crate::ui::widgets::editor_box(editor, || (64.0, 160.0), true).into_any()
        },
    )
    .style(move |s| s.width_full().apply_if(writing.get(), |s| s.hide()))
}

/// The steps, each its own card.
fn steps(
    state: ReviewState,
    backend: &Backend,
    open_file: OpenFile,
    item_id: String,
    walkthrough: Arc<Walkthrough>,
) -> impl IntoView {
    ask_files(state, backend, &item_id, &walkthrough);
    // The background first, unnumbered, then the steps from 1.
    let cards: Vec<(Option<usize>, Step)> = walkthrough
        .background
        .iter()
        .map(|background| (None, background.clone()))
        .chain(
            walkthrough
                .steps
                .iter()
                .enumerate()
                .map(|(i, step)| (Some(i + 1), step.clone())),
        )
        .collect();
    let count = cards.len();
    dyn_stack(
        move || 0..count,
        |i| *i,
        move |i| {
            let (number, step) = cards[i].clone();
            step_card(
                state,
                open_file.clone(),
                item_id.clone(),
                walkthrough.clone(),
                step,
                number,
            )
        },
    )
    .debug_name("walkthrough-steps")
    .style(|s| {
        s.flex_col()
            .width_full()
            .gap(theme::CARD_GAP)
            .padding_top(theme::CARD_GAP)
    })
}

/// Files an item's walkthrough draws its lines from, read at its commits.
/// Another walkthrough of other commits starts them over.
#[derive(Default)]
pub struct WalkthroughFiles {
    pub merge_base: String,
    pub head: String,
    /// Each path asked for, and the answer once there is one.
    pub files: HashMap<String, Option<Result<Arc<LoadedFile>, String>>>,
}

/// Ask for the files the walkthrough's blocks show that haven't been
/// asked for at its commits.
fn ask_files(state: ReviewState, backend: &Backend, item_id: &str, walkthrough: &Walkthrough) {
    let paths: BTreeSet<&str> = walkthrough
        .background
        .iter()
        .chain(&walkthrough.steps)
        .flat_map(|step| &step.body)
        .filter_map(|part| match part {
            Part::Hunk { id, .. } => walkthrough.hunk(id).map(|hunk| hunk.path.as_str()),
            Part::Code(lines) => Some(lines.path.as_str()),
            _ => None,
        })
        .collect();
    let mut wanted = Vec::new();
    state.walkthrough_files.update(|all| {
        let pinned = all.entry(item_id.to_string()).or_default();
        if pinned.merge_base != walkthrough.merge_base || pinned.head != walkthrough.head {
            *pinned = WalkthroughFiles {
                merge_base: walkthrough.merge_base.clone(),
                head: walkthrough.head.clone(),
                files: HashMap::new(),
            };
        }
        for path in paths {
            if !pinned.files.contains_key(path) {
                pinned.files.insert(path.to_string(), None);
                wanted.push(path.to_string());
            }
        }
    });
    if !wanted.is_empty() {
        backend.load_walkthrough_files(item_id, &walkthrough.merge_base, &walkthrough.head, wanted);
    }
}

/// A file read at a walkthrough's commits, kept if the item's walkthrough
/// is still of them.
pub fn file_read(
    review: ReviewState,
    item_id: String,
    (merge_base, head): (String, String),
    path: String,
    file: Result<Arc<LoadedFile>, String>,
) {
    let wanted = review.walkthrough_files.with_untracked(|all| {
        all.get(&item_id)
            .is_some_and(|pinned| pinned.merge_base == merge_base && pinned.head == head)
    });
    if wanted {
        review.walkthrough_files.update(|all| {
            if let Some(pinned) = all.get_mut(&item_id) {
                pinned.files.insert(path, Some(file));
            }
        });
    }
}

/// A step: its number and title, then its body in order, prose with the
/// lines and threads it is about where it set them. The background has
/// no number, and says it is the background over its title.
fn step_card(
    state: ReviewState,
    open_file: OpenFile,
    item_id: String,
    walkthrough: Arc<Walkthrough>,
    step: Step,
    number: Option<usize>,
) -> impl IntoView {
    let title = step.title.clone();
    let eyebrow = Label::derived(|| "Background".to_string()).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(theme::weight_emphasis())
            .color(theme::fg_subtle())
            .selectable(false)
            .apply_if(number.is_some(), |s| s.hide())
    });
    let header = Stack::horizontal((
        Label::derived(move || number.map(|n| n.to_string()).unwrap_or_default()).style(move |s| {
            s.font_size(theme::TEXT_SECTION_TITLE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_subtle())
                .min_width(22.0)
                .flex_shrink(0.0_f32)
                .selectable(false)
                .apply_if(number.is_none(), |s| s.hide())
        }),
        Label::derived(move || title.clone()).style(|s| {
            s.font_size(theme::TEXT_SECTION_TITLE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .text_wrap()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
    ))
    .style(|s| s.items_start().width_full());
    let note = step.unplaced.then(|| {
        Label::derived(|| "The agent didn't place these in its story.".to_string())
            .style(|s| s.font_size(theme::TEXT_BODY).color(theme::fg_muted()))
            .into_any()
    });
    let parts = step.body.iter().filter_map(|part| match part {
        Part::Text { text } => Some(crate::review::markdown::render_linked(text, state).into_any()),
        Part::Hunk { id, start, end } => {
            let hunk = walkthrough.hunk(id)?;
            let block = Block {
                path: hunk.path.clone(),
                side: hunk.side,
                start: start.unwrap_or(hunk.start),
                end: end.unwrap_or(hunk.end),
                context: false,
            };
            Some(block_view(state, open_file.clone(), item_id.clone(), block).into_any())
        }
        Part::Code(lines) => {
            let block = Block {
                path: lines.path.clone(),
                side: lines.side,
                start: lines.start,
                end: lines.end,
                context: true,
            };
            Some(block_view(state, open_file.clone(), item_id.clone(), block).into_any())
        }
        Part::Thread { id } => Some(thread_row(state, id.clone()).into_any()),
        Part::Diagram { source } => Some(crate::review::diagram::view(source.clone()).into_any()),
    });
    let body = Stack::vertical_from_iter(note.into_iter().chain(parts))
        .style(|s| s.flex_col().width_full().gap(10.0));
    Stack::vertical((
        Stack::vertical((eyebrow, header)).style(|s| s.flex_col().width_full().gap(2.0)),
        body,
    ))
    .debug_name(if number.is_some() {
        "walkthrough-step"
    } else {
        "walkthrough-background"
    })
    .style(|s| {
        perch_surface(s)
            .flex_col()
            .width_full()
            .gap(12.0)
            .padding(theme::CARD_PADDING)
    })
}

/// Lines a step shows: a hunk of the change, or lines it cites.
#[derive(Debug, Clone)]
struct Block {
    path: String,
    side: CodeSide,
    start: u32,
    end: u32,
    /// Cited lines rather than a hunk.
    context: bool,
}

/// A block: the file and lines over them, the file opening Code at the
/// first line; then the lines, as the diff tints them, once the file is
/// read at the walkthrough's commits.
fn block_view(
    state: ReviewState,
    open_file: OpenFile,
    item_id: String,
    block: Block,
) -> impl IntoView {
    let path = block.path.clone();
    let line = block.start as usize;
    let where_ = match (block.side, block.context) {
        (CodeSide::Base, _) => format!("lines {}\u{2013}{}, before", block.start, block.end),
        (CodeSide::Head, true) => format!("lines {}\u{2013}{}", block.start, block.end),
        (CodeSide::Head, false) => format!("lines {}\u{2013}{} changed", block.start, block.end),
    };
    let header = Stack::horizontal((
        Label::derived({
            let path = block.path.clone();
            move || path.clone()
        })
        .on_event_stop(el::Click, move |_, _| {
            state.one_move(|| {
                state.show_tab(ItemTab::Code);
                open_file(&path);
            });
            if block.side == CodeSide::Head {
                state.jump_to_line.set(Some(line.saturating_sub(1)));
            }
        })
        .debug_name("walkthrough-path")
        .style(|s| {
            clickable(s)
                .font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .hover(|s| s.color(theme::github()))
        }),
        Label::derived(move || where_.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .gap(10.0)
            .height(28.0)
            .padding_horiz(10.0)
    });
    // Read at the walkthrough's own commits, whose lines these are.
    let read = {
        let item_id = item_id.clone();
        let path = block.path.clone();
        Memo::new(move |_| {
            state.walkthrough_files.with(|all| {
                let read = all.get(&item_id)?.files.get(&path)?.as_ref()?;
                Some(read.as_ref().map(|_| ()).map_err(Clone::clone))
            })
        })
    };
    let anchor = Anchor {
        path: block.path.clone(),
        start_line: block.start,
        line: block.end,
        side: block.side.github(),
        commit_sha: String::new(),
    };
    let lines = dyn_container(
        move || read.get(),
        move |read| match read {
            Some(Ok(())) => {
                let file = state.walkthrough_files.with_untracked(|all| {
                    all.get(&item_id)?.files.get(&anchor.path)?.clone()?.ok()
                });
                let Some(file) = file else {
                    return Empty::new().into_any();
                };
                // A file the change leaves alone has no base of its own
                // to read: its base lines are its head lines.
                let anchor = if file.status.is_none() {
                    Anchor {
                        side: perch_github::Side::Right,
                        ..anchor.clone()
                    }
                } else {
                    anchor.clone()
                };
                let (lines, more) = cards::anchored_lines(&file, &anchor, BLOCK_LINES);
                Stack::vertical((cards::code_lines(lines), cards::more_row(more)))
                    .style(|s| s.flex_col().width_full())
                    .into_any()
            }
            Some(Err(e)) => Label::derived(move || format!("These lines couldn't be read: {e}."))
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_subtle())
                        .padding_horiz(10.0)
                        .padding_vert(6.0)
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
    )
    .style(|s| {
        s.width_full()
            .border_top(1.0)
            .border_color(theme::border_muted())
    });
    Stack::vertical((header, lines))
        .style(block_style)
        .into_any()
}

fn block_style(s: floem::style::Style) -> floem::style::Style {
    s.flex_col()
        .width_full()
        .border(1.0)
        .border_color(theme::border_muted())
        .border_radius(6.0)
}

/// A thread the step names: where it is, who started it and how, opening
/// the thread on Conversation.
fn thread_row(state: ReviewState, id: String) -> impl IntoView {
    let words = {
        let id = id.clone();
        move || {
            state.comment_threads.with(|threads| {
                threads
                    .iter()
                    .find(|t| t.id == id)
                    .map(thread_words)
                    .unwrap_or_default()
            })
        }
    };
    Label::derived(words)
        .on_event_stop(el::Click, move |_, _| {
            state.one_move(|| state.show_tab(ItemTab::Conversation));
            state.focus_card.set(Some(CardId::Thread(id.clone())));
        })
        .debug_name("walkthrough-thread")
        .style(|s| {
            clickable(s)
                .font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .text_ellipsis()
                .width_full()
                .min_width(0.0)
                .hover(|s| s.color(theme::github()))
        })
}

/// `Thread on src/a.rs:40 · ana: Why not…`, from its first comment.
fn thread_words(thread: &CommentThread) -> String {
    let place = thread
        .anchor
        .as_ref()
        .map_or_else(|| "the change".to_string(), Anchor::label);
    let first = thread.messages.first();
    let who = first.map(|m| match &m.author {
        Author::User => "you".to_string(),
        Author::Agent { .. } => "agent".to_string(),
        Author::GitHubUser(login) => login.clone(),
    });
    let said = first
        .map(|m| {
            m.body
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
        })
        .unwrap_or("");
    match who {
        Some(who) => format!("Thread on {place} \u{b7} {who}: {said}"),
        None => format!("Thread on {place}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Command, Hunk, Lines};
    use floem::headless::{HeadlessHarness, TestRoot};
    use perch_github::{Item, ItemCheckout};

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, text))
    }

    fn click_on(harness: &mut HeadlessHarness, id: floem::ViewId) {
        let center = id.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        settle(harness);
    }

    fn settle(harness: &mut HeadlessHarness) {
        harness.rebuild();
        harness.paint();
    }

    fn commands_of(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| commands.try_recv().ok()).collect()
    }

    fn state_on_branch() -> ReviewState {
        let mut item = Item::new("acme/repo", "2026-09-20T09:00:00Z".parse().unwrap());
        item.id = "item-7".into();
        item.checkout = Some(ItemCheckout {
            branch: "feature".into(),
            base_ref: "main".into(),
            head_sha: "head-7".into(),
            path: "/tmp/perch-walkthrough-test".into(),
            owned: false,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        let state = ReviewState::new();
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state
    }

    fn walkthrough(head: &str) -> Walkthrough {
        Walkthrough {
            merge_base: "base-7".into(),
            head: head.into(),
            summary: "Stores each request **once**.".into(),
            background: Some(Step {
                title: "How requests are stored".into(),
                body: vec![
                    Part::Text {
                        text: "Each request is kept once.".into(),
                    },
                    Part::Code(Lines {
                        path: "src/x.rs".into(),
                        side: CodeSide::Base,
                        start: 1,
                        end: 2,
                    }),
                    Part::Code(Lines {
                        path: "src/y.rs".into(),
                        side: CodeSide::Base,
                        start: 2,
                        end: 2,
                    }),
                ],
                unplaced: false,
            }),
            steps: vec![
                Step {
                    title: "Parse the request".into(),
                    body: vec![
                        Part::Text {
                            text: "First it reads the request.".into(),
                        },
                        Part::Hunk {
                            id: "h1".into(),
                            start: Some(1),
                            end: Some(2),
                        },
                        Part::Text {
                            text: "Then the line after.".into(),
                        },
                        Part::Hunk {
                            id: "h1".into(),
                            start: Some(3),
                            end: Some(3),
                        },
                        Part::Code(Lines {
                            path: "src/x.rs".into(),
                            side: CodeSide::Head,
                            start: 4,
                            end: 4,
                        }),
                        Part::Thread { id: "gh-9".into() },
                    ],
                    unplaced: false,
                },
                Step {
                    title: "Store it".into(),
                    body: vec![
                        Part::Text {
                            text: "Then it keeps it.".into(),
                        },
                        Part::Diagram {
                            source: "flowchart LR\n  A[Request] --> B[(Store)]".into(),
                        },
                    ],
                    unplaced: false,
                },
            ],
            hunks: vec![Hunk {
                id: "h1".into(),
                path: "src/x.rs".into(),
                side: CodeSide::Head,
                start: 1,
                end: 3,
            }],
            guidance: String::new(),
        }
    }

    /// src/x.rs, which the change modifies, and src/y.rs, which it leaves
    /// alone.
    fn files() -> (Arc<LoadedFile>, Arc<LoadedFile>) {
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "alpha\nbeta2\ngamma\ndelta\n".into(),
            "alpha\nbeta\ngamma\ndelta\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        // A file the change leaves alone.
        let untouched = crate::backend::build_loaded_file(
            "src/y.rs".into(),
            "use x;\nfn caller() {}\n".into(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        (Arc::new(file), Arc::new(untouched))
    }

    /// The files a walkthrough of `head` draws from, read at its commits.
    fn read_at(state: ReviewState, head: &str) {
        let (x, y) = files();
        state.walkthrough_files.update(|all| {
            all.insert(
                "item-7".into(),
                WalkthroughFiles {
                    merge_base: "base-7".into(),
                    head: head.into(),
                    files: HashMap::from([
                        ("src/x.rs".to_string(), Some(Ok(x))),
                        ("src/y.rs".to_string(), Some(Ok(y))),
                    ]),
                },
            );
        });
    }

    fn thread_on_x() -> CommentThread {
        CommentThread {
            id: "gh-9".into(),
            worktree_id: "item-7".into(),
            origin: perch_github::ThreadOrigin::GitHub,
            github_id: Some(9),
            anchor: Some(Anchor {
                path: "src/x.rs".into(),
                start_line: 2,
                line: 2,
                side: perch_github::Side::Right,
                commit_sha: "head-7".into(),
            }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: jiff::Timestamp::UNIX_EPOCH,
            messages: vec![perch_github::CommentMessage {
                id: "m".into(),
                thread_id: "gh-9".into(),
                origin: perch_github::ThreadOrigin::GitHub,
                author: Author::GitHubUser("ana".into()),
                body: "Why b2?\n\nMore.".into(),
                created_at: jiff::Timestamp::UNIX_EPOCH,
                run: None,
                github_id: Some(9),
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
            }],
        }
    }

    /// The tab asks for what is kept; with nothing kept it offers to
    /// write, writes with the agent picked and the reader's guidance, and
    /// offers Stop while it does.
    #[test]
    fn the_tab_asks_for_whats_kept_then_writes_and_can_stop() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on_branch();
        let (backend, mut commands) = Backend::recording();
        let open_file: OpenFile = Rc::new(|_: &str| {});
        let view = view(app, state, Rc::new(backend), open_file);
        let mut harness = HeadlessHarness::new_with_size(root, view, 1000.0, 800.0);
        settle(&mut harness);
        assert!(commands_of(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::LoadWalkthrough { item_id } if item_id == "item-7")));

        apply(state, "item-7".into(), WalkthroughOutcome::Loaded(None));
        settle(&mut harness);
        assert!(named(harness.root_id(), "walkthrough-guidance").is_some());
        assert!(named(harness.root_id(), "walkthrough-model").is_some());

        // The model picked here and the guidance written go with it.
        state.walkthrough_agent.set(Some((
            perch_agent::Backend::Codex,
            "gpt-5.5".into(),
            perch_agent::Effort::High,
        )));
        state.walkthrough_guidance.update(|drafts| {
            drafts.insert("item-7".into(), "Start from the cache.".into());
        });
        settle(&mut harness);
        assert!(find(harness.root_id(), "Codex GPT 5.5").is_some());
        let write = named(harness.root_id(), "walkthrough-write").unwrap();
        click_on(&mut harness, write);
        assert!(commands_of(&mut commands).iter().any(|c| matches!(
            c,
            Command::WriteWalkthrough { item_id, backend, model, guidance, .. }
                if item_id == "item-7"
                    && *backend == perch_agent::Backend::Codex
                    && model == "gpt-5.5"
                    && guidance == "Start from the cache."
        )));
        assert!(find(harness.root_id(), "Writing the walkthrough").is_some());
        // What the agent does shows as it goes, the call it waits on
        // first.
        progress(
            state,
            "item-7".into(),
            vec![perch_github::RunStep {
                summary: "Read src/cache.rs".into(),
                ok: true,
                note: false,
                pending: true,
                user: false,
            }],
        );
        settle(&mut harness);
        assert!(find(harness.root_id(), "src/cache.rs").is_some());
        assert!(find(harness.root_id(), "Writing the walkthrough").is_none());
        let stop = named(harness.root_id(), "walkthrough-stop").unwrap();
        click_on(&mut harness, stop);
        assert!(commands_of(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::StopWalkthrough { item_id } if item_id == "item-7")));

        apply(state, "item-7".into(), WalkthroughOutcome::Stopped);
        assert!(state
            .walkthrough_steps
            .with_untracked(std::collections::HashMap::is_empty));
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Failed("The agent wrote nothing".into()),
        );
        settle(&mut harness);
        assert!(find(
            harness.root_id(),
            "couldn't be written: The agent wrote nothing"
        )
        .is_some());
    }

    /// A kept walkthrough brings back the guidance it was written with,
    /// unless the reader has written some since.
    #[test]
    fn a_kept_walkthrough_fills_the_guidance_left_empty() {
        let _root = TestRoot::new();
        let state = ReviewState::new();
        let kept = |guidance: &str| Walkthrough {
            guidance: guidance.into(),
            ..walkthrough("head-7")
        };
        apply(
            state,
            "a".into(),
            WalkthroughOutcome::Loaded(Some(kept("Start from the cache."))),
        );
        state.walkthrough_guidance.update(|drafts| {
            drafts.insert("b".into(), "Mine.".into());
        });
        apply(
            state,
            "b".into(),
            WalkthroughOutcome::Written(kept("Start from the cache.")),
        );
        let drafts = state.walkthrough_guidance.get();
        assert_eq!(
            drafts.get("a").map(String::as_str),
            Some("Start from the cache.")
        );
        assert_eq!(drafts.get("b").map(String::as_str), Some("Mine."));
    }

    /// Each step is drawn with its prose, the hunk's lines as the diff
    /// tints them, the cited lines, and its thread, which opens on
    /// Conversation.
    #[test]
    fn steps_draw_their_lines_and_link_their_threads() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on_branch();
        read_at(state, "head-7");
        state.comment_threads.set(Arc::new(vec![thread_on_x()]));
        state.tab.set(ItemTab::Walkthrough);
        let (backend, _commands) = Backend::recording();
        let view = view(app, state, Rc::new(backend), Rc::new(|_: &str| {}));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1000.0, 1400.0);
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        settle(&mut harness);

        // The summary, as markdown, and nothing said under it.
        let summary = named(harness.root_id(), "walkthrough-summary").unwrap();
        assert!(find(summary, "Stores each request").is_some());
        assert!(find(harness.root_id(), "An agent reads the change").is_none());
        // The background first, with its lines from before the change.
        let background = named(harness.root_id(), "walkthrough-background").unwrap();
        assert!(find(background, "How requests are stored").is_some());
        assert!(find(background, "lines 1\u{2013}2, before").is_some());
        assert!(find(background, "alpha").is_some());
        // An unchanged file's base lines are its head's.
        assert!(find(background, "fn caller() {}").is_some());
        assert!(find(harness.root_id(), "Parse the request").is_some());
        assert!(find(harness.root_id(), "Store it").is_some());
        // Text, then the lines it leads into, then text, in that order.
        let top = |text: &str| {
            find(harness.root_id(), text)
                .unwrap_or_else(|| panic!("no {text}"))
                .get_visual_rect_no_clip()
                .y0
        };
        let order = [
            top("First it reads the request."),
            top("lines 1\u{2013}2 changed"),
            top("Then the line after."),
            top("lines 3\u{2013}3 changed"),
            top("lines 4\u{2013}4"),
            top("Thread on src/x.rs:2"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        // The hunk's lines, the removed one among them, and the cited one.
        assert!(find(harness.root_id(), "beta2").is_some());
        assert!(find(harness.root_id(), "delta").is_some());
        assert!(find(harness.root_id(), "lines 4\u{2013}4").is_some());
        assert!(find(harness.root_id(), "Write again").is_some());
        // The diagram is drawn as a figure, in the column's width at most.
        let diagram = named(harness.root_id(), "walkthrough-diagram").unwrap();
        let figure = named(diagram, "figure").expect("drawn, not an error");
        let drawn = figure.children()[0].get_visual_rect_no_clip();
        assert!(drawn.width() > 0.0 && drawn.height() > 0.0, "{drawn:?}");
        assert!(drawn.width() <= figure.get_visual_rect_no_clip().width() + 0.5);

        let thread = find(
            harness.root_id(),
            "Thread on src/x.rs:2 \u{b7} ana: Why b2?",
        )
        .unwrap();
        click_on(&mut harness, thread);
        assert_eq!(state.tab.get_untracked(), ItemTab::Conversation);
        assert_eq!(
            state.focus_card.get_untracked(),
            Some(CardId::Thread("gh-9".into()))
        );
    }

    /// One written for another head says so, and draws its lines as they
    /// were at that head.
    #[test]
    fn a_walkthrough_of_an_older_head_shows_its_lines_as_they_were() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on_branch();
        read_at(state, "old-head");
        let (backend, _commands) = Backend::recording();
        let view = view(app, state, Rc::new(backend), Rc::new(|_: &str| {}));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1000.0, 1400.0);
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Loaded(Some(walkthrough("old-head"))),
        );
        settle(&mut harness);

        assert!(find(harness.root_id(), "Written for old-hea").is_some());
        assert!(find(harness.root_id(), "code is as it was then").is_some());
        assert!(find(harness.root_id(), "Stores each request").is_some());
        assert!(find(harness.root_id(), "Parse the request").is_some());
        assert!(find(harness.root_id(), "beta2").is_some());
        assert!(find(harness.root_id(), "delta").is_some());
    }

    /// Blocks ask for their files at the walkthrough's commits, once, and
    /// draw them as they come, not what Code has loaded. An answer for
    /// other commits is dropped, and one that failed says why.
    #[test]
    fn blocks_read_their_files_at_the_walkthroughs_commits() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = state_on_branch();
        let (x, _) = files();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), x.clone());
        });
        let (backend, mut commands) = Backend::recording();
        let view = view(app, state, Rc::new(backend), Rc::new(|_: &str| {}));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1000.0, 1400.0);
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        settle(&mut harness);

        let asked: Vec<_> = commands_of(&mut commands)
            .into_iter()
            .filter_map(|command| match command {
                Command::LoadWalkthroughFiles {
                    item_id,
                    merge_base,
                    head,
                    paths,
                } => Some((item_id, merge_base, head, paths)),
                _ => None,
            })
            .collect();
        assert_eq!(
            asked,
            vec![(
                "item-7".to_string(),
                "base-7".to_string(),
                "head-7".to_string(),
                vec!["src/x.rs".to_string(), "src/y.rs".to_string()]
            )]
        );
        assert!(find(harness.root_id(), "beta2").is_none());

        file_read(
            state,
            "item-7".into(),
            ("base-7".into(), "old-head".into()),
            "src/x.rs".into(),
            Ok(x.clone()),
        );
        settle(&mut harness);
        assert!(find(harness.root_id(), "beta2").is_none());

        file_read(
            state,
            "item-7".into(),
            ("base-7".into(), "head-7".into()),
            "src/x.rs".into(),
            Ok(x),
        );
        file_read(
            state,
            "item-7".into(),
            ("base-7".into(), "head-7".into()),
            "src/y.rs".into(),
            Err("file is not there".into()),
        );
        settle(&mut harness);
        assert!(find(harness.root_id(), "beta2").is_some());
        assert!(find(harness.root_id(), "couldn't be read: file is not there").is_some());
        assert!(commands_of(&mut commands)
            .iter()
            .all(|c| !matches!(c, Command::LoadWalkthroughFiles { .. })));
    }
}
