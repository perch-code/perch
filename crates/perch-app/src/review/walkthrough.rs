//! Walkthrough: the change told as a tour of the code, as an agent wrote
//! it and the backend's `walkthrough` checked it. Conversation has a card
//! that writes it and says what it is about: the change's summary, and
//! the background to unfold. Start takes the reader to Code, where each
//! step is a card under the lines it is about, as a thread's is, and a
//! bar over the code steps from one to the next. A step's card draws the
//! lines from elsewhere it cites, which Perch reads from the files as a
//! thread's card draws its lines, and the threads it names, which open on
//! Conversation. All of it is Perch's own, so all of it is grey.
//!
//! The lines a step cites are read at the commits the walkthrough was
//! written for, not taken from Code, so they are its lines whatever Code
//! shows. A step's own lines are found in Code's file by their text, so
//! its card follows a file that moved on since, and has none where the
//! lines are gone.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::ui_events::pointer::PointerButton;
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use perch_github::{Anchor, Author, CommentThread, Side};

use crate::backend::{Backend, CodeSide, Part, Step, Walkthrough, WalkthroughOutcome};
use crate::icons::SidebarIcon;
use crate::review::cards::{self, perch_surface, CodeLine, Tint};
use crate::review::doc::CardId;
use crate::review::model::{AgentChoice, ItemTab, LoadedFile, ReviewState, Symbol};
use crate::review::threads::{self, ThreadFace};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{action_button, clickable, icon_action, sidebar_glyph};

/// The most rows a block is tall; a longer range scrolls.
const BLOCK_ROWS: usize = 40;

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

/// The open item's walkthrough, when one is kept for it. Tracked.
pub fn shown(state: ReviewState) -> Option<Arc<Walkthrough>> {
    let id = state.item.with(|i| i.as_ref().map(|i| i.id.clone()))?;
    state
        .walkthroughs
        .with(|kept| kept.get(&id).cloned().flatten())
}

/// Ask for what is kept for the open item, the first time it is wanted.
fn load_kept(state: ReviewState, backend: &Backend) {
    let Some(id) = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
    else {
        return;
    };
    if !state
        .walkthroughs
        .with_untracked(|kept| kept.contains_key(&id))
    {
        backend.load_walkthrough(&id);
    }
}

/// Take the reader to step `index`: Code, its file, its lines a little
/// below the top with its card under them. One move, for Back. The cards
/// come into Code with the first step the reader goes to.
pub fn go_to_step(state: ReviewState, index: usize) {
    let Some(walkthrough) = floem::reactive::Effect::untrack(|| shown(state)) else {
        return;
    };
    let Some(step) = walkthrough.steps.get(index) else {
        return;
    };
    if state.walkthrough_at.get_untracked().is_none() {
        // The rows a card was laid out at on another walkthrough are not
        // this one's.
        state
            .card_rows
            .update(|rows| rows.retain(|id, _| !matches!(id, CardId::Step(_))));
    }
    state.walkthrough_at.set(Some(index));
    // Where its lines are now, if the file is open; else as written.
    let file = state
        .files
        .with_untracked(|files| files.get(&step.at.path).cloned());
    let (first, last) = file
        .as_deref()
        .and_then(|file| Some((file, locate(file, step)?)))
        .map(|(file, anchor)| head_range(file, &anchor))
        .unwrap_or_else(|| match step.at.side {
            CodeSide::Head => (step.at.start as usize, step.at.end as usize),
            CodeSide::Base => (1, 1),
        });
    state.one_move(|| {
        // Through first: an editor already open takes the jump as it is
        // written.
        state.jump_through.set(Some(last.saturating_sub(1)));
        state.go_to_file(&step.at.path, Some((first.saturating_sub(1), 0)));
    });
}

/// The step after the one the reader is on, or before it, if there is
/// one.
pub fn step_by(state: ReviewState, forward: bool) {
    let Some(at) = state.walkthrough_at.get_untracked() else {
        return;
    };
    let count = floem::reactive::Effect::untrack(|| shown(state)).map_or(0, |w| w.steps.len());
    let to = if forward {
        at + 1
    } else {
        match at.checked_sub(1) {
            Some(to) => to,
            None => return,
        }
    };
    if to < count {
        go_to_step(state, to);
    }
}

/// Put the walkthrough away: its cards leave Code, and the bar goes.
pub fn close(state: ReviewState) {
    state.walkthrough_at.set(None);
}

/// The head lines, one-based, an anchor's lines are on or beside: a base
/// line counts as the head line it became.
fn head_range(file: &LoadedFile, anchor: &Anchor) -> (usize, usize) {
    let (start, end) = (anchor.start_line as usize, anchor.line as usize);
    match anchor.side {
        Side::Right => (start, end),
        Side::Left => (head_line_of(file, start), head_line_of(file, end)),
    }
}

/// The lines of one side of a file, in order: the head's, with the
/// removed lines the buffer splices in left out, or the base's. A file
/// the change leaves alone is its head on both sides.
fn side_lines(file: &LoadedFile, side: Side) -> Option<Vec<&str>> {
    match side {
        Side::Right => Some(
            file.buffer
                .text
                .lines()
                .zip(&file.buffer.rows)
                .filter(|(_, row)| !row.is_deleted())
                .map(|(line, _)| line)
                .collect(),
        ),
        Side::Left => Some(file.base_buffer.as_ref()?.text.lines().collect()),
    }
}

/// Where a step's lines are in a file as Code has it: where it says, when
/// they are still there, else the same lines nearest to there, so a step
/// follows a file that moved on since it was written. `None` when the
/// file no longer has them.
pub fn locate(file: &LoadedFile, step: &Step) -> Option<Anchor> {
    let side = side_of(file, step);
    locate_in(file, step, side, &side_lines(file, side)?)
}

/// The side a step's lines are on in a file: its own, but the head's in
/// a file the change leaves alone.
fn side_of(file: &LoadedFile, step: &Step) -> Side {
    if file.status.is_none() {
        Side::Right
    } else {
        step.at.side.github()
    }
}

/// `locate`, over the lines of the side they are on.
fn locate_in(file: &LoadedFile, step: &Step, side: Side, lines: &[&str]) -> Option<Anchor> {
    if step.at.path != file.path {
        return None;
    }
    let (start, end) = (step.at.start as usize, step.at.end as usize);
    let len = end.checked_sub(start)? + 1;
    let same = |at: usize| {
        lines
            .get(at..at + len)
            .is_some_and(|here| here.iter().zip(&step.text).all(|(a, b)| *a == b))
    };
    let found = if step.text.len() != len || same(start - 1) {
        (end <= lines.len()).then_some(start)
    } else {
        (0..=lines.len().saturating_sub(len))
            .filter(|&at| same(at))
            .min_by_key(|&at| at.abs_diff(start - 1))
            .map(|at| at + 1)
    }?;
    Some(Anchor {
        path: file.path.clone(),
        start_line: u32::try_from(found).ok()?,
        line: u32::try_from(found + len - 1).ok()?,
        side,
        commit_sha: String::new(),
    })
}

/// The steps whose cards are in `file`, by index, each with the lines
/// it is under there; none unless the reader is taking the walkthrough.
/// Tracked.
pub fn steps_in(state: ReviewState, file: &LoadedFile) -> Vec<(usize, Anchor)> {
    if state.walkthrough_at.with(Option::is_none) {
        return Vec::new();
    }
    let Some(walkthrough) = shown(state) else {
        return Vec::new();
    };
    // Each side's lines once, however many steps are in the file.
    let head = std::cell::OnceCell::new();
    let base = std::cell::OnceCell::new();
    walkthrough
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.at.path == file.path)
        .filter_map(|(i, step)| {
            let side = side_of(file, step);
            let lines = match side {
                Side::Right => &head,
                Side::Left => &base,
            }
            .get_or_init(|| side_lines(file, side));
            Some((i, locate_in(file, step, side, lines.as_deref()?)?))
        })
        .collect()
}

/// The steps' cards in `file`, each under the last of its lines, with
/// the rows it was laid out at, else an estimate. Tracked.
pub fn step_cards(state: ReviewState, file: &LoadedFile) -> Vec<crate::review::doc::Card> {
    let Some(walkthrough) = shown(state) else {
        return Vec::new();
    };
    steps_in(state, file)
        .into_iter()
        .filter_map(|(i, anchor)| {
            let after_display_line = crate::review::model::card_line(file, &anchor)?;
            let id = CardId::Step(i);
            let rows = state
                .card_rows
                .with(|rows| rows.get(&id).copied())
                .unwrap_or_else(|| estimate_rows(&walkthrough.steps[i]));
            Some(crate::review::doc::Card {
                id,
                after_display_line,
                rows,
                side: anchor.side,
            })
        })
        .collect()
}

/// About how many code rows a step's card is, before it is laid out:
/// the header, the room round the body, and the body's parts, prose at
/// eighty columns. The layout corrects it.
fn estimate_rows(step: &Step) -> usize {
    let body: usize = step
        .body
        .iter()
        .map(|part| match part {
            Part::Text { text } => {
                text.lines()
                    .map(|l| l.chars().count().div_ceil(80).max(1))
                    .sum::<usize>()
                    + 1
            }
            Part::Code(lines) => {
                ((lines.end.saturating_sub(lines.start) + 1) as usize).min(BLOCK_ROWS) + 3
            }
            Part::Thread { .. } => 1,
            Part::Diagram { .. } => 12,
        })
        .sum();
    body + 5
}

/// Step through the walkthrough from Code: where the reader is in it and
/// what the step is, Back and Next, and the way out. Over the code while
/// the reader is taking it; grey, as it is Perch's.
pub fn step_bar(state: ReviewState) -> impl IntoView {
    let count = Memo::new(move |_| shown(state).map_or(0, |w| w.steps.len()));
    let at = Memo::new(move |_| {
        let at = state.walkthrough_at.get()?;
        (at < count.get()).then_some(at)
    });
    let title = Memo::new(move |_| {
        let at = at.get()?;
        shown(state).and_then(|w| w.steps.get(at).map(|s| s.title.clone()))
    });
    let here = Label::derived(move || match at.get() {
        Some(at) => format!("Step {} of {}", at + 1, count.get()),
        None => String::new(),
    })
    .style(|s| {
        s.font_size(theme::TEXT_META)
            .font_weight(theme::weight_emphasis())
            .color(theme::fg_muted())
            .flex_shrink(0.0_f32)
            .selectable(false)
    });
    let name = Label::derived(move || title.get().unwrap_or_default())
        .on_event_stop(el::Click, move |_, _| {
            if let Some(at) = at.get_untracked() {
                go_to_step(state, at);
            }
        })
        .debug_name("walkthrough-bar-title")
        .style(|s| {
            clickable(s)
                .font_size(theme::TEXT_META)
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        });
    let back = cards::tip(
        icon_action(
            SidebarIcon::ChevronUp,
            "walkthrough-back",
            move || at.get().is_some_and(|at| at > 0),
            move || step_by(state, false),
        ),
        format!(
            "Previous step ({})",
            crate::ui::menu::Shortcut::label_of("Comma")
        ),
    );
    let next = cards::tip(
        icon_action(
            SidebarIcon::ChevronDown,
            "walkthrough-next",
            move || at.get().is_some_and(|at| at + 1 < count.get()),
            move || step_by(state, true),
        ),
        format!(
            "Next step ({})",
            crate::ui::menu::Shortcut::label_of("Period")
        ),
    );
    let done = cards::tip(
        icon_action(
            SidebarIcon::Close,
            "walkthrough-close",
            || true,
            move || close(state),
        ),
        "Put the walkthrough away".to_string(),
    );
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Milestone, theme::fg_muted)
            .style(|s| s.size(14.0, 14.0).flex_shrink(0.0_f32)),
        here,
        name,
        back,
        next,
        done,
    ))
    .debug_name("walkthrough-bar")
    .style(move |s| {
        s.items_center()
            .width_full()
            .height(36.0)
            .gap(8.0)
            .padding_left(theme::CODE_GUTTER_GAP + 4.0)
            .padding_right(6.0)
            .background(theme::perch_bg())
            .border_bottom(1.0)
            .border_color(theme::border_muted())
            .apply_if(at.get().is_none(), |s| s.hide())
    })
}

/// The card on Conversation that writes the walkthrough and starts it:
/// what a walkthrough is before there is one, that it is being written,
/// why it could not be, or that it is of an older head; once written, the
/// summary, the background to unfold, and Start. Under that the reader's
/// guidance, the model and effort, and Write, or Write again, or Stop.
/// Only for an item with files.
pub fn conversation_card(
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
            if item.get().is_some() {
                load_kept(state, &backend);
            }
        });
    }
    let shown = Memo::new(move |_| shown(state));
    // Whether it was written for a head other than the checkout's.
    let stale = Memo::new(move |_| {
        let head = state.item.with(|i| {
            i.as_ref()
                .and_then(|i| i.checkout.as_ref())
                .map(|c| c.head_sha.clone())
        });
        shown.with(|w| w.as_ref().zip(head).is_some_and(|(w, head)| w.head != head))
    });
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
            None => "An agent reads the change and tells it as a tour of the code, stop by \
                     stop, in the order that's easiest to follow. Each stop is a card under \
                     its lines in Code, and Perch checks that every hunk has one."
                .to_string(),
            Some(w) if stale.get() => format!(
                "Written for {}, before the latest commits. Steps whose lines have changed \
                 since have no card. Write it again to see the change as it is.",
                crate::review::timeline::short_sha(&w.head)
            ),
            Some(_) => String::new(),
        }
    };
    let title = Stack::horizontal((
        sidebar_glyph(SidebarIcon::Milestone, theme::fg_muted)
            .style(|s| s.size(14.0, 14.0).flex_shrink(0.0_f32)),
        Label::derived(|| "Walkthrough".to_string()).style(|s| {
            s.font_size(theme::TEXT_SECTION_TITLE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .selectable(false)
        }),
        Label::derived(move || {
            shown
                .get()
                .map(|w| match w.steps.len() {
                    1 => "1 step".to_string(),
                    n => format!("{n} steps"),
                })
                .unwrap_or_default()
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .selectable(false)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        action_button(
            move || match state.walkthrough_at.get() {
                Some(at) => format!("Go to step {}", at + 1),
                None => "Start".to_string(),
            },
            "walkthrough-start",
            true,
            || true,
            move || go_to_step(state, state.walkthrough_at.get_untracked().unwrap_or(0)),
        )
        .style(move |s| s.apply_if(shown.with(Option::is_none), |s| s.hide())),
    ))
    .style(|s| s.items_center().width_full().gap(8.0));
    let text = Label::derived(words).style(move |s| {
        s.font_size(theme::TEXT_BODY)
            .color(theme::fg_muted())
            .text_wrap()
            .width_full()
            .apply_if(words().is_empty(), |s| s.hide())
    });
    let story = {
        let backend = backend.clone();
        dyn_container(
            move || shown.get(),
            move |shown| match (shown, item.get_untracked()) {
                (Some(walkthrough), Some(item_id)) => story(
                    app,
                    state,
                    backend.clone(),
                    open_file.clone(),
                    item_id,
                    walkthrough,
                )
                .into_any(),
                _ => Empty::new().into_any(),
            },
        )
        .style(|s| s.width_full())
    };
    // The guidance box is folded away until asked for, or until there is
    // guidance in it, so the card stays short on every item.
    let asked = RwSignal::new(false);
    let guided = Memo::new(move |_| {
        let Some(id) = item.get() else {
            return false;
        };
        asked.get()
            || state
                .walkthrough_guidance
                .with(|drafts| drafts.get(&id).is_some_and(|g| !g.trim().is_empty()))
    });
    let add_guidance = Label::derived(|| "Add guidance".to_string())
        .on_event_stop(el::Click, move |_, _| asked.set(true))
        .debug_name("walkthrough-add-guidance")
        .style(move |s| {
            clickable(s)
                .font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .selectable(false)
                .hover(|s| s.color(theme::fg()))
                .apply_if(guided.get(), |s| s.hide())
        });
    let write_backend = backend.clone();
    let stop_backend = backend.clone();
    let controls = Stack::horizontal((
        agent_picker(app, state, writing),
        effort_picker(app, state, writing),
        add_guidance,
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
        title,
        text,
        story,
        Container::new(guidance_box(app, state, backend, item, writing))
            .style(move |s| s.width_full().apply_if(!guided.get(), |s| s.hide())),
        controls,
        writing_bar(state, item, started, stop_backend),
    ))
    .debug_name("walkthrough-card")
    .style(move |s| {
        perch_surface(s)
            .flex_col()
            .width_full()
            .gap(12.0)
            .padding(theme::CARD_PADDING)
            .margin_bottom(theme::CARD_GAP)
            .apply_if(!state.has_files(), |s| s.hide())
    })
}

/// What a written walkthrough says before its steps: the summary, then
/// the background folded under its title, which a click opens.
fn story(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    item_id: String,
    walkthrough: Arc<Walkthrough>,
) -> impl IntoView {
    ask_files(state, &backend, &item_id, &walkthrough);
    let summary = (!walkthrough.summary.trim().is_empty())
        .then(|| crate::review::markdown::render_linked(&walkthrough.summary, state).into_any());
    let background = walkthrough.background.clone().map(|background| {
        let open = RwSignal::new(false);
        let title = background.title.clone();
        let toggle = Stack::horizontal((
            sidebar_glyph_dyn(move || {
                if open.get() {
                    SidebarIcon::ChevronDown
                } else {
                    SidebarIcon::ChevronRight
                }
            }),
            Label::derived(move || format!("Background: {title}")).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
                    .text_wrap()
                    .min_width(0.0)
                    .flex_grow(1.0_f32)
                    .flex_basis(0.0)
                    .selectable(false)
            }),
        ))
        .on_event_stop(el::Click, move |_, _| open.update(|o| *o = !*o))
        .debug_name("walkthrough-background-toggle")
        .style(|s| clickable(s).items_center().width_full().gap(6.0));
        let key = format!("{}/background", walkthrough.head);
        let body = body_view(
            app,
            state,
            backend.clone(),
            open_file.clone(),
            item_id.clone(),
            walkthrough.head.clone(),
            key,
            &background.body,
        )
        .debug_name("walkthrough-background")
        .style(move |s| s.apply_if(!open.get(), |s| s.hide()));
        Stack::vertical((toggle, body))
            .style(|s| s.flex_col().width_full().gap(10.0))
            .into_any()
    });
    Stack::vertical_from_iter(summary.into_iter().chain(background))
        .debug_name("walkthrough-story")
        .style(|s| s.flex_col().width_full().gap(12.0))
}

/// A chevron that turns as `icon` says.
fn sidebar_glyph_dyn(icon: impl Fn() -> SidebarIcon + Copy + 'static) -> impl IntoView {
    crate::ui::widgets::icon_in(
        move || crate::icons::sidebar_icon(icon(), theme::fg_muted()),
        theme::fg_muted,
    )
    .style(|s| {
        s.size(theme::CHEVRON_GLYPH, theme::CHEVRON_GLYPH)
            .flex_shrink(0.0_f32)
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
        .map(|background| &background.body)
        .chain(walkthrough.steps.iter().map(|step| &step.body))
        .flatten()
        .filter_map(|part| match part {
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

/// A step's card in Code, under its lines: which step it is and its
/// title, then its body, prose with the lines from elsewhere and the
/// threads it needs where it set them. The step the reader is on is
/// framed in the accent and ends in the way to the next; a click on
/// another makes it the one. Grey, as all of the walkthrough is Perch's.
pub fn step_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    index: usize,
) -> impl IntoView {
    let walkthrough = floem::reactive::Effect::untrack(|| shown(state));
    let item_id = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
        .unwrap_or_default();
    let (Some(walkthrough), false) = (walkthrough, item_id.is_empty()) else {
        return Empty::new().into_any();
    };
    let Some(step) = walkthrough.steps.get(index).cloned() else {
        return Empty::new().into_any();
    };
    ask_files(state, &backend, &item_id, &walkthrough);
    let count = walkthrough.steps.len();
    let current = Memo::new(move |_| state.walkthrough_at.get() == Some(index));
    let title = step.title.clone();
    let header = Stack::horizontal((
        Label::derived(move || format!("{} of {count}", index + 1)).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_subtle())
                .flex_shrink(0.0_f32)
                .selectable(false)
        }),
        Label::derived(move || title.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
        }),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .gap(8.0)
            .height(theme::INLINE_THREAD_HEADER)
            .padding_horiz(theme::COLLAPSED_CARD_PADDING)
            .border_bottom(1.0)
            .border_color(theme::border_muted())
    });
    let note = step.unplaced.then(|| {
        Label::derived(|| "The agent didn't place these lines in its story.".to_string())
            .style(|s| s.font_size(theme::TEXT_BODY).color(theme::fg_muted()))
            .into_any()
    });
    let open_file = crate::review::open_file(state, backend.clone());
    let body = body_view(
        app,
        state,
        backend,
        open_file,
        item_id,
        walkthrough.head.clone(),
        format!("{}/{index}", walkthrough.head),
        &step.body,
    );
    // Where to go from here: the next step, by its title, or the end.
    let next_title = walkthrough.steps.get(index + 1).map(|s| s.title.clone());
    let last = next_title.is_none();
    let foot = Stack::horizontal((
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        action_button(
            move || match &next_title {
                Some(title) => format!("Next: {title}"),
                None => "Done".to_string(),
            },
            "walkthrough-step-next",
            false,
            || true,
            move || {
                if last {
                    close(state);
                } else {
                    go_to_step(state, index + 1);
                }
            },
        )
        .style(|s| s.max_width_full().text_ellipsis()),
    ))
    .debug_name("walkthrough-step-foot")
    .style(move |s| {
        s.items_center()
            .width_full()
            .apply_if(!current.get(), |s| s.hide())
    });
    let inside =
        Stack::vertical_from_iter(note.into_iter().chain([body.into_any(), foot.into_any()]))
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .gap(10.0)
                    .padding(theme::COLLAPSED_CARD_PADDING)
            });
    Stack::vertical((header, inside))
        .on_event_cont(el::PointerDown, move |_, _| {
            if !current.get_untracked() {
                state.walkthrough_at.set(Some(index));
            }
        })
        .debug_name("walkthrough-step")
        .style(move |s| {
            s.flex_col()
                .width_full()
                .background(theme::perch_bg())
                .border(1.0)
                .border_radius(theme::RADIUS)
                .border_color(if current.get() {
                    theme::accent()
                } else {
                    theme::border_muted()
                })
        })
        .into_any()
}

/// A body in order: prose, the lines it cites, the threads it names and
/// its diagrams. `key` names it among the walkthrough's bodies, for a
/// composer opened under one of its blocks.
#[allow(clippy::too_many_arguments)]
fn body_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    item_id: String,
    head: String,
    key: String,
    body: &[Part],
) -> Stack {
    // The threads the body names have rows of their own, so a block
    // under them leaves them out.
    let named: Rc<BTreeSet<String>> = Rc::new(
        body.iter()
            .filter_map(|part| match part {
                Part::Thread { id } => Some(id.clone()),
                _ => None,
            })
            .collect(),
    );
    let parts = body.iter().enumerate().map(|(i, part)| match part {
        Part::Text { text } => crate::review::markdown::render_linked(text, state).into_any(),
        Part::Code(lines) => block_view(
            app,
            state,
            backend.clone(),
            open_file.clone(),
            item_id.clone(),
            named.clone(),
            Block {
                key: format!("{key}/{i}"),
                head: head.clone(),
                path: lines.path.clone(),
                side: lines.side,
                start: lines.start,
                end: lines.end,
            },
        )
        .into_any(),
        Part::Thread { id } => thread_row(state, id.clone()).into_any(),
        Part::Diagram { source } => crate::review::diagram::view(source.clone()).into_any(),
    });
    Stack::vertical_from_iter(parts.collect::<Vec<_>>())
        .style(|s| s.flex_col().width_full().gap(10.0))
}

/// Lines a step cites.
#[derive(Debug, Clone)]
struct Block {
    /// Which block of the walkthrough this is, for a composer opened
    /// under it.
    key: String,
    /// The head the walkthrough was written for, whose lines these are.
    head: String,
    path: String,
    side: CodeSide,
    start: u32,
    end: u32,
}

/// A block: the file and lines over them, the file opening Code at the
/// first line, a line from before the change at the head line it became;
/// then the lines, as the diff tints them and on code's white, once the
/// file is read at the walkthrough's commits. A line's `+` asks about it
/// in a composer under the block, as Code's gutter does, and the threads
/// on its lines that the step doesn't name already are listed under it.
/// The whole file scrolls under the lines, and once it has, a button at
/// the right of the header takes it back to them.
fn block_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    item_id: String,
    named: Rc<BTreeSet<String>>,
    block: Block,
) -> impl IntoView {
    // Bumped to scroll back to the block's lines.
    let home = RwSignal::new(0_u64);
    let moved = RwSignal::new(false);
    let path = block.path.clone();
    let where_ = match block.side {
        CodeSide::Base => format!("lines {}\u{2013}{}, before", block.start, block.end),
        CodeSide::Head => format!("lines {}\u{2013}{}", block.start, block.end),
    };
    let header = Stack::horizontal((
        Label::derived({
            let path = block.path.clone();
            move || path.clone()
        })
        .on_event_stop(el::Click, {
            let item_id = item_id.clone();
            let block = block.clone();
            move |_, _| {
                let (start, end) = (block.start as usize, block.end as usize);
                let (first, last) = match block.side {
                    CodeSide::Head => (start, end),
                    CodeSide::Base => state
                        .walkthrough_files
                        .with_untracked(|all| {
                            let file = all.get(&item_id)?.files.get(&path)?.clone()?.ok()?;
                            Some((head_line_of(&file, start), head_line_of(&file, end)))
                        })
                        .unwrap_or((start, end)),
                };
                state.one_move(|| {
                    state.show_tab(ItemTab::Code);
                    open_file(&path);
                });
                // Through first: an editor already open takes the jump
                // as it is written.
                state.jump_through.set(Some(last.saturating_sub(1)));
                state.jump_to_line.set(Some(first.saturating_sub(1)));
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
        icon_action(
            SidebarIcon::Undo,
            "walkthrough-lines-back",
            || true,
            move || home.update(|h| *h += 1),
        )
        .style(move |s| {
            s.size(22.0, 22.0)
                .margin_right(-6.0)
                .apply_if(!moved.get(), |s| s.hide())
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
    let file = {
        let item_id = item_id.clone();
        let path = block.path.clone();
        move || {
            state
                .walkthrough_files
                .with(|all| all.get(&item_id)?.files.get(&path)?.clone())
        }
    };
    let read = {
        let file = file.clone();
        Memo::new(move |_| file().map(|read| read.map(|_| ())))
    };
    let hovered: RwSignal<Option<(Side, usize)>> = RwSignal::new(None);
    let lines = {
        let file = file.clone();
        let block = block.clone();
        let backend = backend.clone();
        dyn_container(
            move || read.get(),
            move |read| match read {
                Some(Ok(())) => {
                    let Some(Ok(file)) = floem::reactive::Effect::untrack(&file) else {
                        return Empty::new().into_any();
                    };
                    let (anchor, lines, range) = block_lines(&file, &block);
                    if lines.is_empty() {
                        return Empty::new().into_any();
                    }
                    // Its head lines are the checkout's, for rust-analyzer
                    // to be asked about, when it was written for its head.
                    let linked = anchor.side == Side::Right
                        && state.item.with_untracked(|i| {
                            i.as_ref()
                                .and_then(|i| i.checkout.as_ref())
                                .is_some_and(|c| c.head_sha == block.head)
                        });
                    let links = linked.then(|| (backend.clone(), block.path.clone()));
                    let hand: RwSignal<Option<usize>> = RwSignal::new(None);
                    let block = block.clone();
                    let row = move |i: usize, (side, line): (Side, CodeLine)| {
                        let link = links.clone().map(|links| Link { links, hand, i });
                        line_row(app, state, &block, &anchor, side, line, hovered, link)
                    };
                    file_scroll(lines, range, home, moved, hand, row).into_any()
                }
                Some(Err(e)) => {
                    Label::derived(move || format!("These lines couldn't be read: {e}."))
                        .style(|s| {
                            s.font_size(theme::TEXT_BADGE)
                                .color(theme::fg_subtle())
                                .padding_horiz(10.0)
                                .padding_vert(6.0)
                        })
                        .into_any()
                }
                None => Empty::new().into_any(),
            },
        )
        .style(|s| {
            s.width_full()
                .border_top(1.0)
                .border_color(theme::border_muted())
                .background(theme::code_bg())
                // Inside the block's frame, its corners less the border.
                .border_bottom_left_radius(5.0)
                .border_bottom_right_radius(5.0)
        })
    };
    // The threads on the block's lines: on these lines at the head the
    // walkthrough was written for, or on the current head's when that is
    // the one, as Code shows them.
    let threads = {
        let block = block.clone();
        Memo::new(move |_| {
            let shown: Vec<(Side, usize)> = match file() {
                Some(Ok(file)) => {
                    let (_, lines, range) = block_lines(&file, &block);
                    lines[range]
                        .iter()
                        .filter_map(|(side, line)| Some((*side, line.number?)))
                        .collect()
                }
                _ => return Vec::new(),
            };
            let current = state.item.with(|i| {
                i.as_ref()
                    .and_then(|i| i.checkout.as_ref())
                    .is_some_and(|c| c.head_sha == block.head)
            });
            state.comment_threads.with(|threads| {
                threads
                    .iter()
                    .filter(|t| !named.contains(&t.id) && state.thread_visible(t))
                    .filter(|t| {
                        t.anchor.as_ref().is_some_and(|a| {
                            a.path == block.path
                                && shown.contains(&(a.side, a.line as usize))
                                && (a.commit_sha == block.head || (current && !t.outdated))
                        })
                    })
                    .map(|t| t.id.clone())
                    .collect::<Vec<_>>()
            })
        })
    };
    let thread_rows = dyn_stack(
        move || threads.get(),
        String::clone,
        move |id| thread_row(state, id),
    )
    .debug_name("walkthrough-block-threads")
    .style(move |s| {
        s.flex_col()
            .width_full()
            .gap(4.0)
            .apply_if(threads.with(Vec::is_empty), |s| s.hide())
    });
    // Keyed on which opening it is, not on the composer, which picking a
    // model writes, so what is typed stays.
    let opening = {
        let key = block.key.clone();
        Memo::new(move |_| {
            state.composer.with(|c| {
                c.as_ref()
                    .filter(|c| c.thread_id.is_none() && c.block.as_ref() == Some(&key))
                    .map(|c| c.opening)
            })
        })
    };
    let composer = dyn_container(
        move || opening.get(),
        move |opening| match (opening, state.composer.get_untracked()) {
            (Some(_), Some(c)) => {
                threads::composer_view(app, state, backend.clone(), c, ThreadFace::Card).into_any()
            }
            _ => Empty::new().into_any(),
        },
    )
    .debug_name("walkthrough-composer")
    .style(move |s| {
        s.width_full()
            .apply_if(opening.with(Option::is_none), |s| s.hide())
    });
    Stack::vertical((
        Stack::vertical((header, lines)).style(block_style),
        thread_rows,
        composer,
    ))
    .style(|s| s.flex_col().width_full().gap(8.0))
    .into_any()
}

/// The lines of a block's file, each with the side it counts on, and
/// where the block's own are among them; and the anchor they are read by.
/// A file the change leaves alone has no base of its own to read: its
/// base lines are its head lines.
fn block_lines(
    file: &LoadedFile,
    block: &Block,
) -> (Anchor, Vec<(Side, CodeLine)>, std::ops::Range<usize>) {
    let side = if file.status.is_none() {
        Side::Right
    } else {
        block.side.github()
    };
    let anchor = Anchor {
        path: block.path.clone(),
        start_line: block.start,
        line: block.end,
        side,
        commit_sha: block.head.clone(),
    };
    let (lines, range) = cards::file_lines(file, &anchor);
    // Lines read from the base are the base's; among the head's, a removed
    // one counts on the base side.
    let lines = lines
        .into_iter()
        .map(|line| {
            let side = if side == Side::Left || line.tint == Tint::Removed {
                Side::Left
            } else {
                Side::Right
            };
            (side, line)
        })
        .collect();
    (anchor, lines, range)
}

/// A file's lines in a box the height of `range`'s, opened on them and
/// scrolling through the rest, as wide as its longest line, each drawn by
/// `row`. `moved` says whether it is somewhere else; a bump of `home`
/// takes it back. `hand` is cleared as the pointer leaves.
fn file_scroll(
    lines: Vec<(Side, CodeLine)>,
    range: std::ops::Range<usize>,
    home: RwSignal<u64>,
    moved: RwSignal<bool>,
    hand: RwSignal<Option<usize>>,
    row: impl Fn(usize, (Side, CodeLine)) -> floem::AnyView + 'static,
) -> impl IntoView {
    use floem::views::scroll::ScrollChanged;

    let start = floem::kurbo::Vec2::new(0.0, range.start as f64 * theme::CODE_ROW_HEIGHT);
    let height = range.len().clamp(1, BLOCK_ROWS) as f64 * theme::CODE_ROW_HEIGHT;
    // The row's own right padding past the longest line.
    let width = lines
        .iter()
        .map(|(_, l)| &l.text)
        .max_by_key(|text| text.chars().count())
        .map_or(0.0, |text| cards::code_line_layout(text).size().width)
        + cards::CODE_TEXT_INSET
        + 12.0;
    let lines = Arc::new(lines);
    floem::views::virtual_stack(
        move || Lines(lines.clone()),
        |(i, _)| *i,
        move |(i, line)| row(i, line),
    )
    .item_size_fixed(|| theme::CODE_ROW_HEIGHT)
    .style(move |s| s.flex_col().min_width_full().width(width))
    .scroll()
    .scroll_to(move || {
        home.track();
        Some(start.to_point())
    })
    .on_event_cont(
        ScrollChanged::listener(),
        move |_, event: &ScrollChanged| {
            let away = (event.offset - start).hypot() > 0.5;
            if moved.get_untracked() != away {
                moved.set(away);
            }
        },
    )
    .on_event_cont(el::PointerLeave, move |_, _| hand.set(None))
    .debug_name("walkthrough-lines")
    .style(move |s| {
        s.width_full()
            .height(height + 1.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}

/// A file's lines as the virtual stack reads them, a slice at a time.
struct Lines(Arc<Vec<(Side, CodeLine)>>);

impl floem::views::VirtualVector<(usize, (Side, CodeLine))> for Lines {
    fn total_len(&self) -> usize {
        self.0.len()
    }

    fn slice(
        &self,
        range: std::ops::Range<usize>,
    ) -> impl Iterator<Item = (usize, (Side, CodeLine))> {
        let start = range.start;
        self.0[range]
            .iter()
            .cloned()
            .enumerate()
            .map(move |(i, line)| (start + i, line))
    }
}

/// What a row of a linked block needs for Cmd-click: the backend and the
/// head file to ask about, the block's hand, and which row this is.
struct Link {
    links: (Rc<Backend>, String),
    hand: RwSignal<Option<usize>>,
    i: usize,
}

/// A line of a block, with the `+` that asks about it under the pointer,
/// as Code's gutter has: about this line, or with Shift from the line a
/// composer under the block starts on to this one. The lines a composer
/// under the block is about are marked. With `link`, the name under the
/// pointer in a head line shows the hand while Cmd is held, and a
/// Cmd-click asks where it is defined, which opens on Code.
#[allow(clippy::too_many_arguments)]
fn line_row(
    app: AppState,
    state: ReviewState,
    block: &Block,
    anchor: &Anchor,
    side: Side,
    line: CodeLine,
    hovered: RwSignal<Option<(Side, usize)>>,
    link: Option<Link>,
) -> floem::AnyView {
    let Some(number) = line.number else {
        return cards::code_row(line).into_any();
    };
    let at = (side, number);
    let key = block.key.clone();
    // The lines of the composer under this block, on this line's side.
    let asked = {
        let key = key.clone();
        move || {
            state.composer.with(|c| {
                c.as_ref()
                    .filter(|c| c.thread_id.is_none() && c.block.as_ref() == Some(&key))
                    .and_then(|c| c.anchor.as_ref())
                    .filter(|a| a.side == side)
                    .map(|a| (a.start_line.min(a.line), a.start_line.max(a.line)))
            })
        }
    };
    let marked = {
        let asked = asked.clone();
        move || asked().is_some_and(|(first, last)| (first..=last).contains(&(number as u32)))
    };
    let anchor = anchor.clone();
    let plus = Container::new(
        sidebar_glyph(SidebarIcon::Plus, theme::canvas).style(|s| s.size(11.0, 11.0)),
    )
    .debug_name("walkthrough-plus")
    .on_event_stop(el::PointerDown, move |_, e: &PointerButtonEvent| {
        if e.button != Some(PointerButton::Primary) {
            return;
        }
        let line = number as u32;
        let from = e
            .state
            .modifiers
            .shift()
            .then(|| floem::reactive::Effect::untrack(&asked))
            .flatten()
            .map(|(first, _)| first);
        let (start, end) = match from {
            Some(first) => (first.min(line), first.max(line)),
            None => (line, line),
        };
        state.open_composer_under(
            key.clone(),
            Anchor {
                start_line: start,
                line: end,
                side,
                ..anchor.clone()
            },
            from.is_some(),
            crate::review::threads::defaults(app),
        );
    })
    .style(move |s| {
        clickable(s)
            .absolute()
            .inset_left(4.0)
            .inset_top((theme::CODE_ROW_HEIGHT - 16.0) / 2.0)
            .size(16.0, 16.0)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .background(theme::accent())
            .apply_if(hovered.get() != Some(at), |s| s.hide())
    });
    let mark = Empty::new().style(move |s| {
        s.absolute()
            .inset(0.0)
            .pointer_events_none()
            .background(theme::selection_bg())
            .apply_if(!marked(), |s| s.hide())
    });
    let i = link.as_ref().map(|l| l.i);
    let hand = link.as_ref().map(|l| l.hand);
    let text = cards::code_row_with(line.clone(), move || {
        hand.is_some_and(|hand| hand.get() == i)
    });
    let row = Stack::new((text, mark, plus))
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(Some(at)))
        .on_event_cont(el::PointerLeave, move |_, _| {
            if hovered.get_untracked() == Some(at) {
                hovered.set(None);
            }
        })
        .style(|s| s.width_full());
    match link {
        Some(link) => linked_row(state, row, line, link),
        None => row.into_any(),
    }
}

/// A row whose names go to their definitions on a Cmd-click.
fn linked_row(
    state: ReviewState,
    row: impl IntoView + 'static,
    line: CodeLine,
    Link {
        links: (backend, path),
        hand,
        i,
    }: Link,
) -> floem::AnyView {
    use crate::review::navigation::command_held;
    use floem::event::EventPropagation;

    let row = row.into_any();
    let row_id = row.id();
    // The name under a window point, in the row's text.
    let symbol_at = move |window_point: floem::kurbo::Point| {
        let origin = row_id.get_visual_rect_no_clip().origin();
        let x = window_point.x - origin.x - cards::CODE_TEXT_INSET;
        name_at(&path, &line, x)
    };
    let move_at = symbol_at.clone();
    row.on_event_cont(el::PointerMove, move |cx, update| {
        let at = cx.world_transform.inverse() * update.current.logical_point();
        let over = command_held(&update.current.modifiers) && move_at(at).is_some();
        let now = over.then_some(i);
        if hand.get_untracked() != now {
            hand.set(now);
        }
    })
    .on_event_with_config(
        el::PointerDown,
        floem::context::EventCallbackConfig {
            phases: floem::context::Phases::CAPTURE,
        },
        move |cx, e: &PointerButtonEvent| {
            if e.button != Some(PointerButton::Primary) || !command_held(&e.state.modifiers) {
                return EventPropagation::Continue;
            }
            let at = cx.world_transform.inverse() * e.state.logical_point();
            match symbol_at(at) {
                Some(symbol) => {
                    crate::review::navigation::go_to_definition(state, &backend, &symbol);
                    EventPropagation::Stop
                }
                None => EventPropagation::Continue,
            }
        },
    )
    .into_any()
}

/// The name `x` across a head line's text is on, as a symbol of the head
/// file `path`. `None` on a removed line, which the server has no place
/// for.
fn name_at(path: &str, line: &CodeLine, x: f64) -> Option<Symbol> {
    use crate::review::references::{identifier_at, is_identifier_char};

    if line.tint == Tint::Removed {
        return None;
    }
    let number = line.number?;
    let byte = cards::code_byte_at(&line.text, x)?;
    if !line.text[byte..]
        .chars()
        .next()
        .is_some_and(is_identifier_char)
    {
        return None;
    }
    let (start, name) = identifier_at(&line.text, byte)?;
    Some(Symbol {
        path: path.to_string(),
        line: number - 1,
        column: start,
        end: start + name.len(),
        text: name.to_string(),
    })
}

/// The head line a base line became, both 1-based: itself in a file the
/// change leaves alone, else its pair's, and for a removed line the head
/// line after it, where Code shows it.
fn head_line_of(file: &LoadedFile, base_line: usize) -> usize {
    if file.status.is_none() {
        return base_line;
    }
    let Some(at) = file
        .alignment
        .iter()
        .position(|(base, _)| *base == Some(base_line))
    else {
        return base_line;
    };
    file.alignment[at..]
        .iter()
        .find_map(|(_, head)| *head)
        .or_else(|| {
            file.alignment[..at]
                .iter()
                .rev()
                .find_map(|(_, head)| *head)
        })
        .unwrap_or(base_line)
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
    use crate::backend::{Background, Command, Hunk, Lines};
    use crate::review::model::History;
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
            background: Some(Background {
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
            }),
            steps: vec![
                Step {
                    title: "Parse the request".into(),
                    at: Lines {
                        path: "src/x.rs".into(),
                        side: CodeSide::Head,
                        start: 2,
                        end: 2,
                    },
                    text: vec!["beta2".into()],
                    body: vec![
                        Part::Text {
                            text: "First it reads the request.".into(),
                        },
                        Part::Code(Lines {
                            path: "src/x.rs".into(),
                            side: CodeSide::Head,
                            start: 1,
                            end: 3,
                        }),
                        Part::Text {
                            text: "Then the line after.".into(),
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
                    at: Lines {
                        path: "src/y.rs".into(),
                        side: CodeSide::Head,
                        start: 2,
                        end: 2,
                    },
                    text: vec!["fn caller() {}".into()],
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

    /// Conversation's card, over a recording backend.
    fn card_harness(
        state: ReviewState,
        height: f64,
    ) -> (
        HeadlessHarness,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let root = TestRoot::new();
        let app = AppState::new();
        let (backend, commands) = Backend::recording();
        let view = conversation_card(app, state, Rc::new(backend), Rc::new(|_: &str| {}));
        let harness = HeadlessHarness::new_with_size(root, view, 1000.0, height);
        (harness, commands)
    }

    /// The steps' cards, one over the other, as Code would draw them
    /// under their lines.
    fn steps_harness(
        state: ReviewState,
    ) -> (
        HeadlessHarness,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let root = TestRoot::new();
        let app = AppState::new();
        let (backend, commands) = Backend::recording();
        let backend = Rc::new(backend);
        let view = Stack::vertical((
            step_card(app, state, backend.clone(), 0),
            step_card(app, state, backend, 1),
        ))
        .style(|s| s.flex_col().width_full().gap(20.0));
        let harness = HeadlessHarness::new_with_size(root, view, 1000.0, 1400.0);
        (harness, commands)
    }

    /// The card asks for what is kept; with nothing kept it offers to
    /// write, writes with the agent picked and the reader's guidance, and
    /// offers Stop while it does.
    #[test]
    fn the_card_asks_for_whats_kept_then_writes_and_can_stop() {
        let state = state_on_branch();
        let (mut harness, mut commands) = card_harness(state, 800.0);
        settle(&mut harness);
        assert!(commands_of(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::LoadWalkthrough { item_id } if item_id == "item-7")));

        apply(state, "item-7".into(), WalkthroughOutcome::Loaded(None));
        settle(&mut harness);
        assert!(named(harness.root_id(), "walkthrough-model").is_some());
        // The guidance box is folded away until asked for.
        let guidance = named(harness.root_id(), "walkthrough-guidance").unwrap();
        let shown = |id: floem::ViewId| id.get_visual_rect_no_clip().height() > 0.0;
        assert!(!shown(guidance));
        let add = named(harness.root_id(), "walkthrough-add-guidance").unwrap();
        click_on(&mut harness, add);
        assert!(shown(guidance));
        assert!(add.is_hidden());
        assert!(find(harness.root_id(), "An agent reads the change").is_some());
        assert!(named(harness.root_id(), "walkthrough-start")
            .unwrap()
            .is_hidden());

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

    /// Written, the card on Conversation says how many steps, shows the
    /// summary, and folds the background under its title until a click
    /// opens it, with its lines from before the change. Start takes the
    /// reader to the first step's lines on Code, as one move, and then
    /// reads as the way back to the step they are on.
    #[test]
    fn the_card_shows_the_summary_and_background_and_starts() {
        let state = state_on_branch();
        read_at(state, "head-7");
        let (mut harness, _commands) = card_harness(state, 1400.0);
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        settle(&mut harness);

        assert!(find(harness.root_id(), "2 steps").is_some());
        assert!(find(harness.root_id(), "Stores each request").is_some());
        assert!(find(harness.root_id(), "An agent reads the change").is_none());
        assert!(find(harness.root_id(), "Write again").is_some());
        let background = named(harness.root_id(), "walkthrough-background").unwrap();
        assert!(background.is_hidden());
        let toggle = named(harness.root_id(), "walkthrough-background-toggle").unwrap();
        assert!(find(toggle, "Background: How requests are stored").is_some());
        click_on(&mut harness, toggle);
        assert!(!background.is_hidden());
        assert!(find(background, "lines 1\u{2013}2, before").is_some());
        assert!(find(background, "alpha").is_some());
        // An unchanged file's base lines are its head's.
        assert!(find(background, "fn caller() {}").is_some());

        let start = named(harness.root_id(), "walkthrough-start").unwrap();
        assert!(find(start, "Start").is_some());
        click_on(&mut harness, start);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(0));
        assert_eq!(state.tab.get_untracked(), ItemTab::Code);
        assert_eq!(
            state.pending_jump.get_untracked(),
            Some(("src/x.rs".to_string(), 1, 0))
        );
        assert_eq!(state.jump_through.get_untracked(), Some(1));
        assert!(state.history.with_untracked(History::can_go_back));
        assert!(find(start, "Go to step 1").is_some());
    }

    /// A step's card says which step it is, then its body in order: the
    /// prose, the lines it cites and the thread it names, which opens on
    /// Conversation. The step the reader is on ends in the way to the
    /// next; the last's puts the walkthrough away. A press on another
    /// step's card makes it the one.
    #[test]
    fn a_step_card_draws_its_body_and_goes_on() {
        let state = state_on_branch();
        read_at(state, "head-7");
        state.comment_threads.set(Arc::new(vec![thread_on_x()]));
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        state.walkthrough_at.set(Some(0));
        let (mut harness, _commands) = steps_harness(state);
        settle(&mut harness);

        assert!(find(harness.root_id(), "1 of 2").is_some());
        assert!(find(harness.root_id(), "Parse the request").is_some());
        // Text, then the lines it leads into, then text, in that order.
        let top = |text: &str| {
            find(harness.root_id(), text)
                .unwrap_or_else(|| panic!("no {text}"))
                .get_visual_rect_no_clip()
                .y0
        };
        let order = [
            top("First it reads the request."),
            top("lines 1\u{2013}3"),
            top("Then the line after."),
            top("lines 4\u{2013}4"),
            top("Thread on src/x.rs:2"),
            top("2 of 2"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        // The cited lines, the removed one among them.
        assert!(find(harness.root_id(), "beta2").is_some());
        assert!(find(harness.root_id(), "delta").is_some());
        // The diagram is drawn as a figure, in the card's width at most.
        let diagram = named(harness.root_id(), "walkthrough-diagram").unwrap();
        let figure = named(diagram, "figure").expect("drawn, not an error");
        let drawn = figure.children()[0].get_visual_rect_no_clip();
        assert!(drawn.width() > 0.0 && drawn.height() > 0.0, "{drawn:?}");
        assert!(drawn.width() <= figure.get_visual_rect_no_clip().width() + 0.5);

        // Only the step the reader is on has the way on.
        let mut feet = Vec::new();
        all_named(harness.root_id(), "walkthrough-step-foot", &mut feet);
        let shown = |feet: &[floem::ViewId]| -> Vec<floem::ViewId> {
            feet.iter()
                .filter(|id| !id.is_hidden())
                .map(|id| named(*id, "walkthrough-step-next").unwrap())
                .collect()
        };
        let next = shown(&feet);
        assert_eq!(next.len(), 1);
        assert!(find(next[0], "Next: Store it").is_some());

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

        // Next goes to the second step's lines, in the file it is in.
        click_on(&mut harness, next[0]);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(1));
        assert_eq!(state.tab.get_untracked(), ItemTab::Code);
        assert_eq!(
            state.pending_jump.get_untracked(),
            Some(("src/y.rs".to_string(), 1, 0))
        );
        let done = shown(&feet);
        assert_eq!(done.len(), 1, "the second step's");
        let done = done[0];
        assert!(find(done, "Done").is_some());

        // A press on the first card makes it the one, with no move.
        state.pending_jump.set(None);
        let first = find(harness.root_id(), "First it reads the request.").unwrap();
        click_on(&mut harness, first);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(0));
        assert_eq!(state.pending_jump.get_untracked(), None);

        state.walkthrough_at.set(Some(1));
        settle(&mut harness);
        click_on(&mut harness, done);
        assert_eq!(state.walkthrough_at.get_untracked(), None);
    }

    /// The primary button going down at a point, with Shift held or not.
    fn press(x: f64, y: f64, shift: bool) -> floem::event::Event {
        use floem::ui_events::keyboard::Modifiers;
        use floem::ui_events::pointer::{
            PointerButtonEvent, PointerEvent, PointerId, PointerInfo, PointerState, PointerType,
        };
        let mut state = PointerState::default();
        state.position.x = x;
        state.position.y = y;
        state.count = 1;
        if shift {
            state.modifiers = Modifiers::SHIFT;
        }
        floem::event::Event::Pointer(PointerEvent::Down(PointerButtonEvent {
            state,
            button: Some(PointerButton::Primary),
            pointer: PointerInfo {
                pointer_id: Some(PointerId::PRIMARY),
                persistent_device_id: None,
                pointer_type: PointerType::Mouse,
            },
        }))
    }

    /// Every view named `name` under `id`.
    fn all_named(id: floem::ViewId, name: &str, found: &mut Vec<floem::ViewId>) {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            found.push(id);
        }
        for child in id.children() {
            all_named(child, name, found);
        }
    }

    /// Point at the first line of the first step showing `text`, and
    /// press its `+`.
    fn ask_on(harness: &mut HeadlessHarness, text: &str, shift: bool) {
        let step = named(harness.root_id(), "walkthrough-step").unwrap();
        let y = find(step, text)
            .unwrap_or_else(|| panic!("no {text}"))
            .get_visual_rect_no_clip()
            .center()
            .y;
        harness.pointer_move(300.0, y);
        settle(harness);
        let mut pluses = Vec::new();
        all_named(harness.root_id(), "walkthrough-plus", &mut pluses);
        let plus = pluses
            .into_iter()
            .map(|id| id.get_visual_rect_no_clip())
            .find(|r| r.width() > 0.0 && r.y0 <= y && y <= r.y1)
            .expect("a + on the line under the pointer");
        let at = plus.center();
        harness.dispatch_event(press(at.x, at.y, shift));
        harness.pointer_up(at.x, at.y);
        settle(harness);
    }

    /// The one block's composer slot that holds a composer.
    fn open_composer(harness: &HeadlessHarness) -> floem::ViewId {
        let mut slots = Vec::new();
        all_named(harness.root_id(), "walkthrough-composer", &mut slots);
        let open: Vec<_> = slots
            .into_iter()
            .filter(|slot| named(*slot, "composer-box").is_some())
            .collect();
        assert_eq!(open.len(), 1, "one block draws the composer");
        open[0]
    }

    /// A block's line offers a `+` under the pointer, which opens a
    /// composer on it under the block, Code staying up, and no card for
    /// it in the code. Shift on another widens it to both and keeps what
    /// was typed. The thread it starts is listed under the block; one the
    /// step names already is not listed twice.
    #[test]
    fn a_blocks_lines_are_asked_about_under_the_block() {
        let state = state_on_branch();
        read_at(state, "head-7");
        let (x, _) = files();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), x);
        });
        state.active.set(Some("src/x.rs".into()));
        state.comment_threads.set(Arc::new(vec![thread_on_x()]));
        state.tab.set(ItemTab::Code);
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        state.walkthrough_at.set(Some(0));
        let (mut harness, _commands) = steps_harness(state);
        settle(&mut harness);
        assert!(named(harness.root_id(), "composer-box").is_none());

        ask_on(&mut harness, "beta2", false);
        let anchor = |state: ReviewState| {
            state
                .composer
                .get_untracked()
                .and_then(|c| c.block.is_some().then_some(c.anchor).flatten())
                .map(|a| (a.path, a.start_line, a.line, a.side, a.commit_sha))
        };
        assert_eq!(
            anchor(state),
            Some((
                "src/x.rs".to_string(),
                2,
                2,
                Side::Right,
                "head-7".to_string()
            ))
        );
        assert_eq!(state.tab.get_untracked(), ItemTab::Code);
        assert!(!state.composer_is_inline());
        assert!(!state
            .cards_in("src/x.rs")
            .iter()
            .any(|card| card.id == CardId::Composer));
        let composer = open_composer(&harness);
        assert!(find(composer, "src/x.rs:2").is_some());

        state.composer_draft.set("Why?".into());
        ask_on(&mut harness, "alpha", true);
        assert_eq!(
            anchor(state),
            Some((
                "src/x.rs".to_string(),
                1,
                2,
                Side::Right,
                "head-7".to_string()
            ))
        );
        assert_eq!(state.composer_draft.get_untracked(), "Why?");
        let composer = open_composer(&harness);
        assert!(find(composer, "src/x.rs:1\u{2013}2").is_some());

        // Sent, the box goes, and the thread it started is under the
        // block, gh-9 only in the step's own row.
        let mut asked = thread_on_x();
        asked.id = "local-1".into();
        asked.origin = perch_github::ThreadOrigin::Local;
        asked.github_id = None;
        asked.anchor = Some(Anchor {
            start_line: 1,
            ..asked.anchor.clone().unwrap()
        });
        asked.messages[0].author = Author::User;
        asked.messages[0].body = "Why?".into();
        state.composer_sent();
        state
            .comment_threads
            .set(Arc::new(vec![thread_on_x(), asked]));
        settle(&mut harness);
        assert!(named(harness.root_id(), "composer-box").is_none());
        let mut listed = Vec::new();
        all_named(harness.root_id(), "walkthrough-block-threads", &mut listed);
        let listed: Vec<_> = listed
            .into_iter()
            .filter(|id| !id.children().is_empty())
            .collect();
        assert_eq!(listed.len(), 1, "only the block it was asked from");
        assert!(find(listed[0], "you: Why?").is_some());
        assert!(find(listed[0], "ana").is_none());

        // A removed line asks about the base's.
        ask_on(&mut harness, "beta", false);
        assert_eq!(
            anchor(state).map(|(_, start, line, side, _)| (start, line, side)),
            Some((2, 2, Side::Left))
        );
    }

    /// A block opens on its lines and scrolls through the rest of its
    /// file; once moved, the button in its header takes it back. A
    /// Cmd-click on a name in a head line asks for its definition, which
    /// opens on Code.
    #[test]
    fn a_block_scrolls_its_file_and_goes_to_definitions() {
        // A click is turned into a name by the code's widths.
        crate::theme::install_fonts_for_tests();
        let state = state_on_branch();
        read_at(state, "head-7");
        state.set_server_status(
            perch_lsp::ServerKind::RustAnalyzer,
            perch_lsp::Status::Ready,
        );
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        state.walkthrough_at.set(Some(0));
        let (mut harness, mut commands) = steps_harness(state);
        settle(&mut harness);

        // The block citing line 4, one row tall, on `delta`.
        let cited = find(harness.root_id(), "lines 4\u{2013}4")
            .unwrap()
            .get_visual_rect_no_clip();
        let below = |name: &str| {
            let mut ids = Vec::new();
            all_named(harness.root_id(), name, &mut ids);
            ids.into_iter()
                .filter(|id| id.get_visual_rect_no_clip().y1 > cited.y0)
                .min_by(|a, b| {
                    let y = |id: &floem::ViewId| id.get_visual_rect_no_clip().y0;
                    y(a).total_cmp(&y(b))
                })
                .unwrap()
        };
        let lines = below("walkthrough-lines");
        // Hidden, it has no place to look for it by: it is in the block.
        let block = lines.parent().and_then(|p| p.parent()).unwrap();
        let back = named(block, "walkthrough-lines-back").unwrap();
        let rect = lines.get_visual_rect_no_clip();
        assert_eq!(
            rect.height(),
            theme::CODE_ROW_HEIGHT + 1.0,
            "a row and its rule"
        );
        let on = |text: &str| {
            find(lines, text).is_some_and(|id| rect.contains(id.get_visual_rect_no_clip().center()))
        };
        assert!(on("delta") && !on("gamma"));
        assert!(back.is_hidden(), "nothing to go back to");

        // Up a line, to `gamma`, and the button shows; it goes back.
        harness.scroll_up(rect.center().x, rect.center().y, theme::CODE_ROW_HEIGHT);
        settle(&mut harness);
        assert!(on("gamma") && !on("delta"));
        assert!(!back.is_hidden());
        click_on(&mut harness, back);
        settle(&mut harness);
        assert!(on("delta"));
        assert!(back.is_hidden());

        // Cmd on `delta`, head line 4, asks where it is defined.
        commands_of(&mut commands);
        let at = rect.x0 + cards::CODE_TEXT_INSET + 4.0;
        crate::review::navigation::tests::cmd_click_at(&mut harness, at, rect.center().y);
        settle(&mut harness);
        let asked: Vec<_> = commands_of(&mut commands)
            .into_iter()
            .filter_map(|command| match command {
                Command::Definition {
                    token,
                    path,
                    line,
                    character,
                } => Some((token, path, line, character)),
                _ => None,
            })
            .collect();
        let [(token, path, line, character)] = asked.as_slice() else {
            panic!("one definition asked: {asked:?}");
        };
        assert_eq!((path.as_str(), *line, *character), ("src/x.rs", 3, 0));
        assert_eq!(state.tab.get_untracked(), ItemTab::Conversation);
        let place = crate::review::code_refs::located("src/x.rs", 2, 0);
        state.finish_goto(*token, Ok(vec![place]));
        assert_eq!(state.tab.get_untracked(), ItemTab::Code);
        assert_eq!(
            state.pending_jump.get_untracked(),
            Some(("src/x.rs".to_string(), 1, 0))
        );
    }

    /// A line from before the change is found where it is after: moved
    /// down by what was put in above it, and a removed one at the line
    /// that follows it. A file the change leaves alone is the same on
    /// both sides.
    #[test]
    fn a_base_line_maps_to_the_head_line_it_became() {
        let file = crate::backend::build_loaded_file(
            "src/z.rs".into(),
            "new\nalso new\na\nc\nd\n".into(),
            "a\ngone\nc\nd\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        assert_eq!(head_line_of(&file, 1), 3);
        assert_eq!(head_line_of(&file, 2), 4, "removed: the line after it");
        assert_eq!(head_line_of(&file, 3), 4);
        assert_eq!(head_line_of(&file, 4), 5);
        let (_, untouched) = files();
        assert_eq!(head_line_of(&untouched, 2), 2);
    }

    /// A step's lines are where it says while they are still there; in a
    /// file that has moved on, the same lines nearest to there; and
    /// nowhere once they are gone. Removed lines are found on the base,
    /// and an unchanged file's base is its head.
    #[test]
    fn a_step_finds_its_lines_in_a_file_that_moved() {
        let (x, y) = files();
        let step = &walkthrough("head-7").steps[0];
        let at = |file: &LoadedFile, step: &Step| {
            locate(file, step).map(|a| (a.start_line, a.line, a.side))
        };
        assert_eq!(at(&x, step), Some((2, 2, Side::Right)));
        assert_eq!(at(&y, step), None, "another file");

        let moved = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "x\nx\nx\nalpha\nbeta2\ngamma\nx\nx\nx\nbeta2\n".into(),
            "alpha\nbeta\ngamma\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        assert_eq!(at(&moved, step), Some((5, 5, Side::Right)), "the nearer");
        let gone = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "alpha\nbeta3\n".into(),
            "alpha\nbeta\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        assert_eq!(at(&gone, step), None);

        let removed = Step {
            at: Lines {
                path: "src/x.rs".into(),
                side: CodeSide::Base,
                start: 2,
                end: 2,
            },
            text: vec!["beta".into()],
            ..step.clone()
        };
        assert_eq!(at(&x, &removed), Some((2, 2, Side::Left)));
        let unchanged = Step {
            at: Lines {
                path: "src/y.rs".into(),
                side: CodeSide::Base,
                start: 1,
                end: 1,
            },
            text: vec!["use x;".into()],
            ..step.clone()
        };
        assert_eq!(at(&y, &unchanged), Some((1, 1, Side::Right)));
    }

    /// While the reader takes the walkthrough, each file has a card for
    /// each step in it, right under the step's lines and over a thread's
    /// there, and the folds keep its lines in view; put away, none.
    #[test]
    fn steps_have_cards_in_code_while_the_walkthrough_is_out() {
        let _root = TestRoot::new();
        let state = state_on_branch();
        let (x, y) = files();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), x.clone());
            files.insert("src/y.rs".into(), y);
        });
        state.comment_threads.set(Arc::new(vec![thread_on_x()]));
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        let ids = |path: &str| -> Vec<CardId> {
            state.cards_in(path).into_iter().map(|c| c.id).collect()
        };
        assert_eq!(ids("src/x.rs"), vec![CardId::Thread("gh-9".into())]);
        assert!(ids("src/y.rs").is_empty());
        assert!(!state.anchors_in("src/y.rs").contains(&(1, 1)));

        state.walkthrough_at.set(Some(0));
        assert_eq!(
            ids("src/x.rs"),
            vec![CardId::Step(0), CardId::Thread("gh-9".into())]
        );
        assert_eq!(ids("src/y.rs"), vec![CardId::Step(1)]);
        assert!(state.anchors_in("src/y.rs").contains(&(1, 1)));
        let card = state.cards_in("src/x.rs").remove(0);
        assert_eq!(
            Some(card.after_display_line),
            crate::review::model::card_line(
                &x,
                &Anchor {
                    path: "src/x.rs".into(),
                    start_line: 2,
                    line: 2,
                    side: Side::Right,
                    commit_sha: String::new(),
                }
            )
        );
        assert_eq!(card.side, Side::Right);
        assert!(card.rows >= 5);

        close(state);
        assert_eq!(ids("src/x.rs"), vec![CardId::Thread("gh-9".into())]);
    }

    /// The bar over the code says where the reader is and what the step
    /// is; it steps on and back, with no back from the first step and no
    /// next from the last, and puts the walkthrough away. Hidden while
    /// it is away.
    #[test]
    fn the_bar_steps_through_the_walkthrough() {
        let root = TestRoot::new();
        let state = state_on_branch();
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        let mut harness = HeadlessHarness::new_with_size(root, step_bar(state), 1000.0, 40.0);
        settle(&mut harness);
        let bar = named(harness.root_id(), "walkthrough-bar").unwrap();
        assert!(bar.is_hidden());

        go_to_step(state, 0);
        settle(&mut harness);
        assert!(!bar.is_hidden());
        assert!(find(bar, "Step 1 of 2").is_some());
        assert!(find(bar, "Parse the request").is_some());
        let back = named(bar, "walkthrough-back").unwrap();
        let next = named(bar, "walkthrough-next").unwrap();
        click_on(&mut harness, back);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(0));
        click_on(&mut harness, next);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(1));
        assert!(find(bar, "Step 2 of 2").is_some());
        assert!(find(bar, "Store it").is_some());
        click_on(&mut harness, next);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(1));
        click_on(&mut harness, back);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(0));

        // Each step is a place to come back to.
        assert_eq!(
            state.pending_jump.get_untracked(),
            Some(("src/x.rs".to_string(), 1, 0))
        );
        let close = named(bar, "walkthrough-close").unwrap();
        click_on(&mut harness, close);
        assert_eq!(state.walkthrough_at.get_untracked(), None);
        assert!(bar.is_hidden());
    }

    /// One written for another head says so, and finds its steps' lines
    /// where they are now.
    #[test]
    fn a_walkthrough_of_an_older_head_says_so() {
        let state = state_on_branch();
        read_at(state, "old-head");
        let (mut harness, _commands) = card_harness(state, 1400.0);
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Loaded(Some(walkthrough("old-head"))),
        );
        settle(&mut harness);

        assert!(find(harness.root_id(), "Written for old-hea").is_some());
        assert!(find(harness.root_id(), "have no card").is_some());
        assert!(find(harness.root_id(), "Stores each request").is_some());
    }

    /// Blocks ask for their files at the walkthrough's commits, once, and
    /// draw them as they come, not what Code has loaded. An answer for
    /// other commits is dropped, and one that failed says why.
    #[test]
    fn blocks_read_their_files_at_the_walkthroughs_commits() {
        let state = state_on_branch();
        let (x, _) = files();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), x.clone());
        });
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(walkthrough("head-7")),
        );
        state.walkthrough_at.set(Some(0));
        let (mut harness, mut commands) = steps_harness(state);
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
        settle(&mut harness);
        assert!(find(harness.root_id(), "beta2").is_some());
        assert!(commands_of(&mut commands)
            .iter()
            .all(|c| !matches!(c, Command::LoadWalkthroughFiles { .. })));
    }
}
