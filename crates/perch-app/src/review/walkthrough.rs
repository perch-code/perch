//! Walkthrough: the change told as a tour of the code, as an agent wrote
//! it and the backend's `walkthrough` checked it. It lives in the drawer
//! on Code, in the tree's place: a tile beside the tree's opens its pane.
//! The pane writes one for the scope Code shows, says what it is about,
//! the summary and the background to unfold, and lists its chapters and
//! their places, each with its Read check. While the pane is out, each
//! place has a note in Code under its lines, drawn as a thread of
//! Perch's own is, which ends in the Follow up box and the way to the
//! place before and after. All of it is Perch's own, so all of it is
//! grey.
//!
//! A place's lines are found in Code's file by their text, so its note
//! follows a file that moved on since, and has none where the lines are
//! gone. The lines a note cites are links that open them in Code.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, ScrollExt, Stack};
use perch_git::Scope;
use perch_github::{Anchor, Author, CommentThread, Side};

use crate::backend::{Backend, CodeSide, Lines, Part, Step, Walkthrough, WalkthroughOutcome};
use crate::icons::SidebarIcon;
use crate::review::cards;
use crate::review::doc::CardId;
use crate::review::model::{AgentChoice, ItemTab, LoadedFile, ReviewState};
use crate::review::threads::{self, Place, ThreadFace};
use crate::state::{AppState, Checked};
use crate::theme;
use crate::ui::drawer;
use crate::ui::widgets::{action_button, checkbox, clickable, icon_action, sidebar_glyph};

/// Fold the backend's answer into the item's walkthroughs.
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
    let open = review
        .item
        .with_untracked(|i| i.as_ref().is_some_and(|i| i.id == item_id));
    match outcome {
        WalkthroughOutcome::Loaded(kept) => {
            if let Some(newest) = kept.first() {
                remember(review, &item_id, &newest.guidance);
            }
            review.walkthroughs.update(|walkthroughs| {
                walkthroughs.insert(item_id, Arc::new(kept.into_iter().map(Arc::new).collect()));
            })
        }
        WalkthroughOutcome::Written(walkthrough) => {
            done(review, &item_id);
            remember(review, &item_id, &walkthrough.guidance);
            review.walkthrough_errors.update(|errors| {
                errors.remove(&item_id);
            });
            review.walkthrough_picked.update(|picked| {
                picked.remove(&item_id);
            });
            if open {
                review.walkthrough_composing.set(false);
                review.walkthrough_at.set(None);
            }
            review.walkthroughs.update(|walkthroughs| {
                let kept = walkthroughs.entry(item_id).or_default();
                let mut all = (**kept).clone();
                all.insert(0, Arc::new(*walkthrough));
                *kept = Arc::new(all);
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

/// The open item's id. Tracked.
fn item_id(state: ReviewState) -> Option<String> {
    state.item.with(|i| i.as_ref().map(|i| i.id.clone()))
}

/// Whether a walkthrough can be written for `scope`: not for Unstaged or
/// Staged, which move as the reader stages.
pub fn writable(scope: &Scope) -> bool {
    !matches!(scope, Scope::Unstaged | Scope::Staged)
}

/// How the pane names a scope in a sentence: "the branch".
fn scope_words(scope: &Scope) -> String {
    match scope {
        Scope::Branch => "the branch".into(),
        Scope::SinceLastLooked { .. } => "what came since you last looked".into(),
        Scope::Commit { sha } => format!("commit {}", crate::review::timeline::short_sha(sha)),
        Scope::Uncommitted | Scope::Unstaged | Scope::Staged => "the uncommitted work".into(),
        Scope::Run { .. } => "what the run changed".into(),
    }
}

/// Ask for a walkthrough of the open item under the scope Code shows,
/// with the agent picked and the reader's guidance.
fn write(app: AppState, state: ReviewState, backend: &Backend) {
    let Some(item_id) = floem::reactive::Effect::untrack(|| item_id(state)) else {
        return;
    };
    let scope = state.scope.get_untracked();
    if !writable(&scope) {
        return;
    }
    let choice = floem::reactive::Effect::untrack(|| choice(app, state));
    let guidance = state
        .walkthrough_guidance
        .with_untracked(|drafts| drafts.get(&item_id).cloned())
        .unwrap_or_default();
    state.agent_last.set(Some(choice.clone()));
    state.walkthroughs_writing.update(|writing| {
        writing.insert(item_id.clone(), (jiff::Timestamp::now(), scope.clone()));
    });
    state.walkthrough_steps.update(|steps| {
        steps.remove(&item_id);
    });
    state.walkthrough_errors.update(|errors| {
        errors.remove(&item_id);
    });
    backend.write_walkthrough(&item_id, &scope, choice, &guidance);
}

/// Every walkthrough kept for the open item, the newest first. Tracked.
fn kept(state: ReviewState) -> Arc<Vec<Arc<Walkthrough>>> {
    let Some(id) = item_id(state) else {
        return Arc::default();
    };
    state
        .walkthroughs
        .with(|kept| kept.get(&id).cloned())
        .unwrap_or_default()
}

/// The open item's walkthroughs of the scope Code shows, the newest
/// first. Tracked.
fn runs(state: ReviewState) -> Vec<Arc<Walkthrough>> {
    let scope = state.scope.get();
    kept(state)
        .iter()
        .filter(|w| w.scope == scope)
        .cloned()
        .collect()
}

/// The walkthrough the pane shows: the one picked from its list, else
/// the newest of the scope Code shows. Tracked.
pub fn shown(state: ReviewState) -> Option<Arc<Walkthrough>> {
    let runs = runs(state);
    let picked = item_id(state).and_then(|id| {
        state
            .walkthrough_picked
            .with(|picked| picked.get(&id).cloned())
    });
    picked
        .and_then(|picked| runs.iter().find(|w| w.id == picked).cloned())
        .or_else(|| runs.first().cloned())
}

/// Ask for what is kept for the open item, the first time it is wanted.
fn load_kept(state: ReviewState, backend: &Backend) {
    let Some(id) = floem::reactive::Effect::untrack(|| item_id(state)) else {
        return;
    };
    if !state
        .walkthroughs
        .with_untracked(|kept| kept.contains_key(&id))
    {
        backend.load_walkthrough(&id);
    }
}

/// Keep places of `walkthrough` read, or not: here at once, and where it
/// is kept.
fn mark_read(
    state: ReviewState,
    backend: &Backend,
    walkthrough: &Walkthrough,
    places: Vec<usize>,
    read: bool,
) {
    let Some(item_id) = floem::reactive::Effect::untrack(|| item_id(state)) else {
        return;
    };
    let changed: Vec<usize> = places
        .into_iter()
        .filter(|p| *p < walkthrough.steps.len() && walkthrough.read.contains(p) != read)
        .collect();
    if changed.is_empty() {
        return;
    }
    let id = walkthrough.id.clone();
    state.walkthroughs.update(|all| {
        let Some(kept) = all.get_mut(&item_id) else {
            return;
        };
        let mut list = (**kept).clone();
        if let Some(found) = list.iter_mut().find(|w| w.id == id) {
            let mut changed_one = (**found).clone();
            for place in &changed {
                if read {
                    changed_one.read.insert(*place);
                } else {
                    changed_one.read.remove(place);
                }
            }
            *found = Arc::new(changed_one);
        }
        *kept = Arc::new(list);
    });
    backend.mark_walkthrough_read(&item_id, &id, changed, read);
}

/// Take the reader to place `index`: Code, its file, its lines a little
/// below the top with its note under them. One move, for Back.
pub fn go_to_step(state: ReviewState, index: usize) {
    let Some(walkthrough) = floem::reactive::Effect::untrack(|| shown(state)) else {
        return;
    };
    let Some(step) = walkthrough.steps.get(index) else {
        return;
    };
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
        if state.tab.get_untracked() != ItemTab::Code {
            state.show_tab(ItemTab::Code);
        }
        // Through first: an editor already open takes the jump as it is
        // written.
        state.jump_through.set(Some(last.saturating_sub(1)));
        state.go_to_file(&step.at.path, Some((first.saturating_sub(1), 0)));
    });
}

/// The place after the one the reader is on, which leaving marks read,
/// or the one before it; the first when they have not started.
pub fn step_by(state: ReviewState, backend: &Backend, forward: bool) {
    let Some(walkthrough) = floem::reactive::Effect::untrack(|| shown(state)) else {
        return;
    };
    let count = walkthrough.steps.len();
    match state
        .walkthrough_at
        .get_untracked()
        .filter(|at| *at < count)
    {
        None if forward && count > 0 => go_to_step(state, 0),
        None => {}
        Some(at) if forward => {
            mark_read(state, backend, &walkthrough, vec![at], true);
            if at + 1 < count {
                go_to_step(state, at + 1);
            }
        }
        Some(at) => {
            if let Some(to) = at.checked_sub(1) {
                go_to_step(state, to);
            }
        }
    }
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

/// Where a place's lines are in a file as Code has it: where it says,
/// when they are still there, else the same lines nearest to there, so a
/// place follows a file that moved on since it was written. `None` when
/// the file no longer has them.
pub fn locate(file: &LoadedFile, step: &Step) -> Option<Anchor> {
    let side = side_of(file, step);
    locate_in(file, step, side, &side_lines(file, side)?)
}

/// The side a place's lines are on in a file: its own, but the head's in
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

/// The places whose notes are in `file`, by index, each with the lines
/// it is under there; none while the pane is shut. Tracked.
pub fn steps_in(state: ReviewState, file: &LoadedFile) -> Vec<(usize, Anchor)> {
    if !state.walkthrough_out.get() {
        return Vec::new();
    }
    let Some(walkthrough) = shown(state) else {
        return Vec::new();
    };
    // Each side's lines once, however many places are in the file.
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

/// The places' notes in `file`, each under the last of its lines, with
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

/// About how many code rows a note is, before it is laid out: the
/// header, the byline, the foot and the room round them, and the body's
/// parts, prose at seventy columns. The layout corrects it.
fn estimate_rows(step: &Step) -> usize {
    let body: usize = step
        .body
        .iter()
        .map(|part| match part {
            Part::Text { text } => {
                text.lines()
                    .map(|l| l.chars().count().div_ceil(70).max(1))
                    .sum::<usize>()
                    + 1
            }
            Part::Code(_) | Part::Thread { .. } => 1,
            Part::Diagram { .. } => 12,
        })
        .sum();
    body + 7
}

/// What the pane's body shows.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    /// What is kept has not come yet.
    Loading,
    Writing,
    /// The form that writes one: none is there for the scope, or the
    /// reader asked for another.
    Form,
    /// The walkthrough with this id.
    Outline(String),
}

/// The walkthrough in the drawer: its bar, with the list of the scope's
/// walkthroughs, Write again and the way to shut it; the scope, as the
/// tree has it; then the form, the agent writing, or the walkthrough's
/// outline. Out, it puts the notes in Code.
pub fn pane(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let item = Memo::new(move |_| item_id(state));
    {
        let backend = backend.clone();
        Effect::new(move |_| {
            if item.get().is_some() {
                load_kept(state, &backend);
            }
        });
    }
    let writing_for = Memo::new(move |_| {
        let id = item.get()?;
        state
            .walkthroughs_writing
            .with(|writing| writing.get(&id).cloned())
    });
    let mode = Memo::new(move |_| {
        if writing_for.with(Option::is_some) {
            return Mode::Writing;
        }
        let loaded = item
            .get()
            .is_some_and(|id| state.walkthroughs.with(|kept| kept.contains_key(&id)));
        if !loaded {
            return Mode::Loading;
        }
        if state.walkthrough_composing.get() {
            return Mode::Form;
        }
        match shown(state) {
            Some(w) => Mode::Outline(w.id.clone()),
            None => Mode::Form,
        }
    });
    let again = cards::tip(
        icon_action(
            SidebarIcon::Refresh,
            "walkthrough-again",
            move || matches!(mode.get(), Mode::Outline(_)),
            move || state.walkthrough_composing.set(true),
        ),
        "Write another".to_string(),
    );
    let header = drawer::header_bar(
        Empty::new(),
        || "Walkthrough".to_string(),
        runs_button(app, state),
        Stack::horizontal((
            again,
            drawer::close_button(app, backend.clone(), "walkthrough-close Close walkthrough"),
        ))
        .style(|s| s.items_center().gap(6.0)),
    );
    let body_backend = backend.clone();
    let body = dyn_container(
        move || mode.get(),
        move |mode| match mode {
            Mode::Loading => Empty::new().into_any(),
            Mode::Writing => writing_view(state, item, writing_for, body_backend.clone()),
            Mode::Form => form_view(app, state, body_backend.clone(), item),
            Mode::Outline(id) => {
                match floem::reactive::Effect::untrack(|| shown(state)).filter(|w| w.id == id) {
                    Some(walkthrough) => {
                        outline(app, state, body_backend.clone(), walkthrough).into_any()
                    }
                    None => Empty::new().into_any(),
                }
            }
        },
    )
    .style(|s| {
        s.flex_col()
            .width_full()
            .max_width_full()
            .padding_bottom(16.0)
    })
    .scroll()
    .class(crate::scrollers::ToEdge)
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0));
    Stack::vertical((header, crate::review::scope::button(state, backend), body))
        .debug_name("walkthrough-panel")
        .style(|s| {
            s.size_full()
                .flex_col()
                .min_height(0.0)
                .background(theme::rail_bg())
        })
}

/// The list of the scope's walkthroughs, at the bar's right: when the one
/// shown was written, and a menu of the rest with Write another. Gone
/// while there is none.
fn runs_button(app: AppState, state: ReviewState) -> impl IntoView {
    use crate::ui::menu::Menu;

    let some = Memo::new(move |_| !runs(state).is_empty());
    crate::review::review_launch::menu_button(
        move || {
            shown(state)
                .and_then(|w| w.written_at)
                .map(|at| crate::timefmt::relative(at, app.now.get()))
                .unwrap_or_default()
        },
        move || {
            let (runs, shown_id) = floem::reactive::Effect::untrack(|| {
                (runs(state), shown(state).map(|w| w.id.clone()))
            });
            let now = jiff::Timestamp::now();
            let mut menu = Menu::new();
            for run in runs {
                let when = run
                    .written_at
                    .map(|at| crate::timefmt::relative(at, now))
                    .unwrap_or_else(|| "Some time ago".into());
                let label = match run.model.as_str() {
                    "" => when,
                    model => format!("{when} \u{b7} {model}"),
                };
                let detail = match floem::reactive::Effect::untrack(|| behind(state, &run)) {
                    Some(behind) => behind,
                    None => crate::review::timeline::short_sha(&run.head),
                };
                let checked = shown_id.as_deref() == Some(run.id.as_str());
                let id = run.id.clone();
                menu = menu.item(label, move |item| {
                    item.checked(checked).detail(detail).action(move || {
                        if let Some(item_id) = floem::reactive::Effect::untrack(|| item_id(state)) {
                            state.walkthrough_picked.update(|picked| {
                                picked.insert(item_id, id.clone());
                            });
                        }
                        state.walkthrough_at.set(None);
                        state.walkthrough_composing.set(false);
                    })
                });
            }
            menu.separator().item("Write another", move |item| {
                item.action(move || state.walkthrough_composing.set(true))
            })
        },
        "walkthrough-runs".to_string(),
    )
    .style(move |s| s.apply_if(!some.get(), |s| s.hide()))
}

/// How far behind the checkout a walkthrough of the branch is: "2 commits
/// behind", or "an older head" for one the log no longer has. `None` for
/// one of the head, and for the other scopes, which are what they are.
fn behind(state: ReviewState, walkthrough: &Walkthrough) -> Option<String> {
    if walkthrough.scope != Scope::Branch {
        return None;
    }
    let head = state.item.with(|i| {
        i.as_ref()
            .and_then(|i| i.checkout.as_ref())
            .map(|c| c.head_sha.clone())
    })?;
    if head == walkthrough.head {
        return None;
    }
    let newer = state
        .commits
        .with(|commits| commits.iter().position(|c| c.sha == walkthrough.head));
    Some(match newer {
        Some(1) => "1 commit behind".to_string(),
        Some(n) => format!("{n} commits behind"),
        None => "an older head".to_string(),
    })
}

/// Muted words in the pane, wrapping.
fn pane_text(words: impl Fn() -> String + 'static) -> Label {
    Label::derived(words).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg_muted())
            .text_wrap()
            .width_full()
            .min_width(0.0)
    })
}

/// A link in the pane's words.
fn pane_link(words: impl Fn() -> String + 'static, debug: &'static str) -> Label {
    Label::derived(words).debug_name(debug).style(|s| {
        clickable(s)
            .font_size(theme::TEXT_META)
            .color(theme::accent())
            .selectable(false)
            .hover(|s| s.color(theme::fg()))
    })
}

/// While one is written: for which scope, what the agent is doing, how
/// long it has gone, and Stop, over the steps it has taken.
fn writing_view(
    state: ReviewState,
    item: Memo<Option<String>>,
    writing_for: Memo<Option<(jiff::Timestamp, Scope)>>,
    backend: Rc<Backend>,
) -> floem::AnyView {
    let started = Memo::new(move |_| writing_for.get().map(|(at, _)| at));
    Stack::vertical((
        pane_text(move || {
            let scope = writing_for.get().map(|(_, s)| s).unwrap_or(Scope::Branch);
            format!(
                "Writing a walkthrough of {}. You can keep reviewing while it writes.",
                scope_words(&scope)
            )
        }),
        writing_bar(state, item, started, backend),
    ))
    .style(|s| s.flex_col().width_full().gap(8.0).padding(12.0))
    .into_any()
}

/// The form that writes one for the scope Code shows: what a walkthrough
/// is, why the last one couldn't be written, the reader's guidance, the
/// model and effort, and Write. Back to the one shown, when there is one;
/// the branch's, from a scope that has none.
fn form_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    item: Memo<Option<String>>,
) -> floem::AnyView {
    let able = Memo::new(move |_| writable(&state.scope.get()));
    let error = Memo::new(move |_| {
        let id = item.get()?;
        state
            .walkthrough_errors
            .with(|errors| errors.get(&id).cloned())
    });
    let never = Memo::new(|_| false);
    let intro = pane_text(move || {
        let scope = state.scope.get();
        if !writable(&scope) {
            return "A walkthrough tells the uncommitted work as a whole. Pick Uncommitted \
                    to write one; Unstaged and Staged move as you stage."
                .to_string();
        }
        format!(
            "An agent reads {} and tells it as a story: a summary, the background, then \
             chapters with a note on each place in the code that matters. It takes a few \
             minutes.",
            scope_words(&scope)
        )
    });
    let failed = Label::derived(move || {
        error
            .get()
            .map(|e| format!("The walkthrough couldn't be written: {e}"))
            .unwrap_or_default()
    })
    .style(move |s| {
        s.font_size(theme::TEXT_META)
            .color(theme::alert_caution())
            .text_wrap()
            .width_full()
            .min_width(0.0)
            .apply_if(error.with(Option::is_none), |s| s.hide())
    });
    let write_backend = backend.clone();
    let controls = Stack::horizontal((
        agent_picker(app, state, never),
        effort_picker(app, state, never),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        action_button(
            || "Write".to_string(),
            "walkthrough-write",
            true,
            || true,
            move || write(app, state, &write_backend),
        )
        .style(|s| s.flex_shrink(0.0_f32)),
    ))
    .style(|s| s.items_center().width_full().gap(6.0));
    let form = Stack::vertical((
        guidance_box(app, state, backend.clone(), item, never),
        controls,
    ))
    .style(move |s| {
        s.flex_col()
            .width_full()
            .gap(8.0)
            .apply_if(!able.get(), |s| s.hide())
    });
    let back_shown =
        Memo::new(move |_| state.walkthrough_composing.get() && !runs(state).is_empty());
    let back = pane_link(|| "Back to the walkthrough".to_string(), "walkthrough-back")
        .on_event_stop(el::Click, move |_, _| {
            state.walkthrough_composing.set(false)
        })
        .style(move |s| s.apply_if(!back_shown.get(), |s| s.hide()));
    // The branch's own, from a scope that has none.
    let branch = Memo::new(move |_| {
        if state.scope.get() == Scope::Branch {
            return None;
        }
        kept(state)
            .iter()
            .find(|w| w.scope == Scope::Branch)
            .cloned()
    });
    let branch_backend = backend.clone();
    let to_branch = Stack::vertical((
        pane_text(move || match branch.get() {
            Some(w) => match behind(state, &w) {
                Some(behind) => format!("The branch has a walkthrough, {behind}."),
                None => "The branch has a walkthrough.".to_string(),
            },
            None => String::new(),
        }),
        pane_link(|| "Show it".to_string(), "walkthrough-to-branch").on_event_stop(
            el::Click,
            move |_, _| {
                if let Some(id) = floem::reactive::Effect::untrack(|| item_id(state)) {
                    branch_backend.set_scope(&id, Scope::Branch);
                }
            },
        ),
    ))
    .style(move |s| {
        s.flex_col()
            .width_full()
            .gap(4.0)
            .padding_top(8.0)
            .border_top(1.0)
            .border_color(theme::border_muted())
            .apply_if(branch.with(Option::is_none), |s| s.hide())
    });
    Stack::vertical((intro, failed, form, back, to_branch))
        .debug_name("walkthrough-form")
        .style(|s| s.flex_col().width_full().gap(10.0).padding(12.0))
        .into_any()
}

/// A walkthrough's outline: how far behind it is, the summary and the
/// background, each folded under its title, Start until the reader has,
/// how many places are read, then the chapters, each folded over its
/// places. A click on a place goes to it.
fn outline(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    walkthrough: Arc<Walkthrough>,
) -> impl IntoView {
    let id = walkthrough.id.clone();
    // The same walkthrough as the reader marks it read.
    let live = {
        let id = id.clone();
        Memo::new(move |_| shown(state).filter(|w| w.id == id))
    };
    let read_set = Memo::new(move |_| {
        live.with(|w| w.as_ref().map(|w| w.read.clone()))
            .unwrap_or_default()
    });
    // The head moves, and the commits arrive, after the outline is built.
    let behind_words = {
        let walkthrough = walkthrough.clone();
        Memo::new(move |_| behind(state, &walkthrough))
    };
    let stale = pane_text({
        let head = crate::review::timeline::short_sha(&walkthrough.head);
        move || {
            format!(
                "Written for {head}, {}. A note whose lines have changed since isn't shown.",
                behind_words.get().unwrap_or_default()
            )
        }
    })
    .style(move |s| {
        s.padding_horiz(12.0)
            .padding_top(10.0)
            .apply_if(behind_words.with(Option::is_none), |s| s.hide())
    });

    let summary = (!walkthrough.summary.trim().is_empty()).then(|| {
        section(
            "Summary",
            true,
            crate::review::markdown::render_linked(&walkthrough.summary, state).into_any(),
        )
    });
    let background = walkthrough.background.clone().map(|background| {
        let body = body_view(state, &background.body);
        section(&background.title, false, body.into_any())
    });

    let first_unread = {
        let count = walkthrough.steps.len();
        move || {
            read_set
                .with(|read| (0..count).find(|i| !read.contains(i)))
                .unwrap_or(0)
        }
    };
    let start = Container::new(action_button(
        move || {
            if read_set.with(BTreeSet::is_empty) {
                "Start".to_string()
            } else {
                "Go on".to_string()
            }
        },
        "walkthrough-start",
        true,
        || true,
        move || go_to_step(state, first_unread()),
    ))
    .style(move |s| {
        s.padding_horiz(12.0)
            .padding_vert(4.0)
            .apply_if(state.walkthrough_at.get().is_some(), |s| s.hide())
    });

    let total = walkthrough.steps.len();
    let all_read = move || read_set.with(|read| read.len() >= total);
    let read_backend = backend.clone();
    let read_row = Stack::horizontal((
        Label::derived(move || format!("{} of {total} read", read_set.with(BTreeSet::len))).style(
            |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_muted())
                    .flex_grow(1.0_f32)
                    .selectable(false)
            },
        ),
        Label::derived(move || {
            if all_read() {
                "Clear all".to_string()
            } else {
                "Mark all".to_string()
            }
        })
        .on_event_stop(el::Click, move |_, _| {
            if let Some(w) = live.get_untracked() {
                mark_read(state, &read_backend, &w, (0..total).collect(), !all_read());
            }
        })
        .debug_name("walkthrough-mark-all")
        .style(|s| {
            clickable(s)
                .font_size(theme::TEXT_BADGE)
                .color(theme::accent())
                .selectable(false)
                .padding_horiz(6.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .hover(|s| s.background(theme::badge_bg()))
        }),
    ))
    .style(|s| {
        s.height(theme::VIEWED_ROW_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_left(12.0)
            .padding_right(4.0)
            .border_top(1.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // Open: the chapter of the place the reader is on, else the first.
    let open: RwSignal<BTreeSet<usize>> = RwSignal::new(
        state
            .walkthrough_at
            .get_untracked()
            .and_then(|at| walkthrough.steps.get(at).map(|s| s.chapter))
            .into_iter()
            .chain(std::iter::once(0))
            .take(1)
            .collect(),
    );
    {
        let walkthrough = walkthrough.clone();
        Effect::new(move |_| {
            if let Some(chapter) = state
                .walkthrough_at
                .get()
                .and_then(|at| walkthrough.steps.get(at).map(|s| s.chapter))
            {
                if !open.with_untracked(|open| open.contains(&chapter)) {
                    open.update(|open| {
                        open.insert(chapter);
                    });
                }
            }
        });
    }
    let chapters = (0..walkthrough.chapters.len()).map(|chapter| {
        chapter_view(
            app,
            state,
            backend.clone(),
            walkthrough.clone(),
            chapter,
            live,
            read_set,
            open,
        )
        .into_any()
    });

    let top = Stack::vertical_from_iter(summary.into_iter().chain(background))
        .style(|s| s.flex_col().width_full().padding_vert(4.0));
    Stack::vertical((
        stale,
        top,
        start,
        read_row,
        Stack::vertical_from_iter(chapters.collect::<Vec<_>>())
            .style(|s| s.flex_col().width_full().padding_vert(4.0)),
    ))
    .debug_name("walkthrough-outline")
    .style(|s| s.flex_col().width_full())
}

/// A part of the outline folded under its title: the summary, the
/// background.
fn section(title: &str, open: bool, body: floem::AnyView) -> impl IntoView {
    let open = RwSignal::new(open);
    let title = title.to_string();
    let toggle = Stack::horizontal((
        chevron(move || open.get()),
        Label::derived(move || title.clone()).style(|s| {
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
    .on_event_stop(el::Click, move |_, _| open.update(|o| *o = !*o))
    .debug_name("walkthrough-section")
    .style(|s| {
        clickable(s)
            .items_center()
            .width_full()
            .gap(4.0)
            .height(theme::TREE_ROW_HEIGHT)
            .padding_horiz(8.0)
            .hover(|s| s.background(theme::canvas_hover()))
    });
    let body = Container::new(body).style(move |s| {
        s.width_full()
            .min_width(0.0)
            .padding_left(26.0)
            .padding_right(12.0)
            .padding_bottom(8.0)
            .apply_if(!open.get(), |s| s.hide())
    });
    Stack::vertical((toggle, body)).style(|s| s.flex_col().width_full())
}

/// A chevron that points down while `open`, else right.
fn chevron(open: impl Fn() -> bool + Copy + 'static) -> impl IntoView {
    crate::ui::widgets::icon_in(
        move || {
            let icon = if open() {
                SidebarIcon::ChevronDown
            } else {
                SidebarIcon::ChevronRight
            };
            crate::icons::sidebar_icon(icon, theme::fg_subtle())
        },
        theme::fg_subtle,
    )
    .style(|s| {
        s.size(14.0, 14.0)
            .flex_shrink(0.0_f32)
            .margin_right(4.0)
            .pointer_events_none()
    })
}

/// One chapter: its title, a caution when a place in it is one to weigh,
/// how many places the follow-through holds, and its check, which marks
/// them all; then its places while it is open.
#[allow(clippy::too_many_arguments)]
fn chapter_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    walkthrough: Arc<Walkthrough>,
    chapter: usize,
    live: Memo<Option<Arc<Walkthrough>>>,
    read_set: Memo<BTreeSet<usize>>,
    open: RwSignal<BTreeSet<usize>>,
) -> impl IntoView {
    let places: Vec<usize> = walkthrough.places_of(chapter).map(|(i, _)| i).collect();
    let info = &walkthrough.chapters[chapter];
    let title = format!("{} {}", chapter + 1, info.title);
    let weigh = !info.follow_through && walkthrough.places_of(chapter).any(|(_, s)| s.weigh);
    let follow = info.follow_through;
    let count = places.len();
    let is_open = move || open.with(|open| open.contains(&chapter));
    let here = {
        let places = places.clone();
        move || {
            state
                .walkthrough_at
                .get()
                .is_some_and(|at| places.contains(&at))
        }
    };
    let all_read = {
        let places = places.clone();
        move || read_set.with(|read| places.iter().all(|p| read.contains(p)))
    };
    let check = {
        let places = places.clone();
        let all_read = all_read.clone();
        let backend = backend.clone();
        checkbox(
            {
                let all_read = all_read.clone();
                move || {
                    if all_read() {
                        Checked::Full
                    } else {
                        Checked::Empty
                    }
                }
            },
            move || {
                if let Some(w) = live.get_untracked() {
                    mark_read(state, &backend, &w, places.clone(), !all_read());
                }
            },
        )
    };
    let header = Stack::horizontal((
        chevron(is_open),
        Label::derived(move || title.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .font_weight(if here() {
                    theme::weight_emphasis()
                } else {
                    theme::weight_body()
                })
                .color(if follow {
                    theme::fg_muted()
                } else {
                    theme::fg()
                })
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .selectable(false)
        }),
        caution(weigh),
        Label::derived(move || count.to_string()).style(move |s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .margin_right(6.0)
                .apply_if(!follow, |s| s.hide())
        }),
        check,
    ))
    .on_event_stop(el::Click, move |_, _| {
        open.update(|open| {
            if !open.remove(&chapter) {
                open.insert(chapter);
            }
        })
    })
    .debug_name(format!("walkthrough-chapter:{}", chapter + 1))
    .style(|s| {
        clickable(s)
            .items_center()
            .width_full()
            .height(theme::TREE_ROW_HEIGHT)
            .padding_left(8.0)
            .padding_right(4.0)
            .hover(|s| s.background(theme::canvas_hover()))
    });
    let rows = places
        .iter()
        .map(|&i| {
            place_row(
                app,
                state,
                backend.clone(),
                &walkthrough.steps[i],
                i,
                live,
                read_set,
            )
            .into_any()
        })
        .collect::<Vec<_>>();
    let body = Stack::vertical_from_iter(rows)
        .style(move |s| s.flex_col().width_full().apply_if(!is_open(), |s| s.hide()));
    Stack::vertical((header, body)).style(|s| s.flex_col().width_full())
}

/// The caution beside a chapter or a place a reviewer should weigh.
fn caution(shown: bool) -> impl IntoView {
    cards::tip(
        sidebar_glyph(SidebarIcon::Warning, theme::alert_warning).style(move |s| {
            s.size(13.0, 13.0)
                .flex_shrink(0.0_f32)
                .margin_right(6.0)
                .apply_if(!shown, |s| s.hide())
        }),
        "Worth a close look".to_string(),
    )
}

/// One place: its title over where it is, its caution, and its Read
/// check. A click goes to it; the place the reader is on is lit.
fn place_row(
    _app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    step: &Step,
    index: usize,
    live: Memo<Option<Arc<Walkthrough>>>,
    read_set: Memo<BTreeSet<usize>>,
) -> impl IntoView {
    let title = step.title.clone();
    let place = where_of(&step.at);
    let read = move || read_set.with(|read| read.contains(&index));
    let here = move || state.walkthrough_at.get() == Some(index);
    let words = Stack::vertical((
        Label::derived(move || title.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if read() {
                    theme::fg_subtle()
                } else {
                    theme::fg()
                })
                .font_weight(if here() {
                    theme::weight_emphasis()
                } else {
                    theme::weight_body()
                })
                .text_ellipsis()
                .min_width(0.0)
                .width_full()
                .selectable(false)
        }),
        Label::derived(move || place.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(theme::fg_subtle())
                .text_ellipsis()
                .min_width(0.0)
                .width_full()
                .selectable(false)
        }),
    ))
    .style(|s| {
        s.flex_col()
            .gap(1.0)
            .min_width(0.0)
            .flex_grow(1.0_f32)
            .flex_basis(0.0)
    });
    let check = checkbox(
        move || {
            if read() {
                Checked::Full
            } else {
                Checked::Empty
            }
        },
        move || {
            if let Some(w) = live.get_untracked() {
                mark_read(state, &backend, &w, vec![index], !read());
            }
        },
    );
    Stack::horizontal((words, caution(step.weigh), check))
        .on_event_stop(el::Click, move |_, _| go_to_step(state, index))
        .debug_name(format!("walkthrough-place:{}", index + 1))
        .style(move |s| {
            let s = clickable(s)
                .items_center()
                .width_full()
                .padding_vert(4.0)
                .padding_left(26.0)
                .padding_right(4.0)
                .hover(|s| s.background(theme::canvas_hover()));
            if here() {
                s.background(theme::accent_subtle())
            } else {
                s
            }
        })
}

/// Where lines are, as a place's row and a note's link say it:
/// `builder.rs:1710`, with "before" for the base's lines.
fn where_of(lines: &Lines) -> String {
    let name = lines.path.rsplit('/').next().unwrap_or(&lines.path);
    match lines.side {
        CodeSide::Head => format!("{name}:{}", lines.start),
        CodeSide::Base => format!("{name}:{} before", lines.start),
    }
}

/// A place's note in Code, under its lines, drawn as a thread of Perch's
/// own is: a header with the lines and the chapter, then the note under
/// the walkthrough's mark, which place of how many it is, and Perch only;
/// the lines it cites, as links, and the threads it names. Its foot is
/// the Follow up box, which opens the composer in its place, as under a
/// thread, and asks about the lines in a thread of their own with the
/// note quoted, and the way to the place before and after, which waits
/// while the composer is open; going on marks it read. The place the
/// reader is on is framed in the accent, and a click on another makes it
/// the one.
pub fn step_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    index: usize,
) -> impl IntoView {
    let walkthrough = floem::reactive::Effect::untrack(|| shown(state));
    let Some(walkthrough) = walkthrough else {
        return Empty::new().into_any();
    };
    let Some(step) = walkthrough.steps.get(index).cloned() else {
        return Empty::new().into_any();
    };
    let count = walkthrough.steps.len();
    let current = Memo::new(move |_| state.walkthrough_at.get() == Some(index));
    // Where its lines are in Code's file now, which a follow-up asks
    // about.
    let anchor = state
        .files
        .with_untracked(|files| files.get(&step.at.path).cloned())
        .and_then(|file| locate(&file, &step));
    let chapter = walkthrough
        .chapters
        .get(step.chapter)
        .map(|c| format!("{} \u{b7} {}", step.chapter + 1, c.title))
        .unwrap_or_default();

    let header = Stack::horizontal((
        lines_heading(anchor.as_ref()),
        Empty::new().style(|s| s.flex_grow(1.0_f32).min_width(8.0)),
        crate::ui::widgets::badge(
            || "Worth a close look".to_string(),
            theme::notice_fg,
            theme::notice_bg,
            Some(theme::notice_border),
        )
        .style(move |s| s.margin_right(6.0).apply_if(!step.weigh, |s| s.hide())),
        crate::ui::widgets::badge(
            move || chapter.clone(),
            theme::fg_muted,
            theme::canvas,
            Some(theme::border),
        )
        .style(|s| s.max_width(220.0).flex_shrink(1.0_f32).min_width(0.0)),
    ))
    .debug_name("walkthrough-note-header")
    .style(|s| {
        s.items_center()
            .width_full()
            .height(theme::INLINE_THREAD_HEADER)
            .padding_horiz(theme::COLLAPSED_CARD_PADDING)
            .border_bottom(1.0)
            .border_color(theme::border_muted())
    });

    let avatar = Container::new(
        sidebar_glyph(SidebarIcon::Milestone, theme::fg_muted).style(|s| s.size(12.0, 12.0)),
    )
    .debug_name("walkthrough-avatar")
    .style(|s| {
        s.size(theme::INLINE_THREAD_AVATAR, theme::INLINE_THREAD_AVATAR)
            .border_radius(theme::INLINE_THREAD_AVATAR / 2.0)
            .background(theme::badge_bg())
            .items_center()
            .justify_center()
            .flex_shrink(0.0_f32)
    });
    let byline = threads::byline_row(
        Label::derived(|| "Walkthrough".to_string())
            .style(|s| threads::name_style(s, ThreadFace::Inline)),
        vec![
            threads::perch_only_label().into_any(),
            Label::derived(move || format!("{} of {count}", index + 1))
                .debug_name("walkthrough-note-count")
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_muted())
                        .margin_left(6.0)
                        .selectable(false)
                })
                .into_any(),
        ],
        Empty::new().into_any(),
    );
    let unplaced = step.unplaced.then(|| {
        Label::derived(|| "The agent didn't place these lines in its story.".to_string())
            .style(|s| s.font_size(theme::TEXT_META).color(theme::fg_muted()))
            .into_any()
    });
    let body = Stack::vertical_from_iter(
        unplaced
            .into_iter()
            .chain([body_view(state, &step.body).into_any()]),
    )
    .style(|s| s.flex_col().width_full().gap(8.0));
    let note = Container::new(threads::comment_frame(
        ThreadFace::Inline,
        Place {
            first: true,
            last: true,
        },
        avatar.into_any(),
        byline,
        body.into_any(),
    ))
    .style(|s| s.width_full().padding_horiz(theme::COLLAPSED_CARD_PADDING));

    // The composer opened here, by which opening it is, so it is built
    // afresh when opened again and not when its destination is picked.
    let walkthrough_id = walkthrough.id.clone();
    let place = (walkthrough_id.clone(), index);
    let composing = Memo::new(move |_| {
        state.composer.with(|c| {
            c.as_ref()
                .filter(|c| {
                    c.step
                        .as_ref()
                        .is_some_and(|(id, at)| *id == walkthrough_id && *at == index)
                })
                .map(|c| c.opening)
        })
    });
    let has_lines = anchor.is_some();
    let reply_backend = backend.clone();
    let follow_up = dyn_container(
        move || composing.get(),
        move |opening| match (opening, state.composer.get_untracked()) {
            (Some(_), Some(composer)) => threads::composer_view(
                app,
                state,
                reply_backend.clone(),
                composer,
                ThreadFace::Inline,
            )
            .style(|s| s.width_full())
            .into_any(),
            _ => follow_up_box(app, state, place.clone(), anchor.clone()).into_any(),
        },
    )
    .style(move |s| {
        s.flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .apply_if(!has_lines, |s| s.hide())
    });
    let back = cards::tip(
        nav_button(
            SidebarIcon::ChevronLeft,
            "walkthrough-note-back",
            move || index > 0,
            {
                let backend = backend.clone();
                move || {
                    state.walkthrough_at.set(Some(index));
                    step_by(state, &backend, false)
                }
            },
        ),
        format!(
            "Previous place ({})",
            crate::ui::menu::Shortcut::label_of("Comma")
        ),
    );
    let next = cards::tip(
        nav_button(
            SidebarIcon::ChevronRight,
            "walkthrough-note-next",
            || true,
            {
                let backend = backend.clone();
                move || {
                    state.walkthrough_at.set(Some(index));
                    step_by(state, &backend, true)
                }
            },
        ),
        if index + 1 < count {
            format!(
                "Next place ({})",
                crate::ui::menu::Shortcut::label_of("Period")
            )
        } else {
            "Mark it read: it's the last".to_string()
        },
    );
    let way_on = Stack::horizontal((back, next))
        .style(move |s| s.gap(6.0).apply_if(composing.get().is_some(), |s| s.hide()));
    let foot = Stack::horizontal((follow_up, way_on))
        .debug_name("walkthrough-note-foot")
        .style(|s| {
            s.items_center()
                .width_full()
                .gap(6.0)
                .padding_horiz(theme::COLLAPSED_CARD_PADDING)
                .padding_bottom(theme::COLLAPSED_CARD_PADDING)
                .padding_top(4.0)
        });

    Stack::vertical((header, note, foot))
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

/// The Follow up box in a note's foot, as a thread's: a click opens the
/// composer in its place, for a thread on the place's lines.
fn follow_up_box(
    app: AppState,
    state: ReviewState,
    step: (String, usize),
    anchor: Option<Anchor>,
) -> impl IntoView {
    Label::derived(|| "Follow up\u{2026}".to_string())
        .on_event_stop(el::Click, move |_, _| {
            if let Some(anchor) = anchor.clone() {
                state.walkthrough_at.set(Some(step.1));
                state.open_step_composer(step.clone(), anchor, threads::defaults(app));
            }
        })
        .debug_name("walkthrough-follow-up")
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_subtle())
                .width_full()
                .height(theme::THREAD_CONTROL - 4.0)
                .padding_horiz(12.0)
                .items_center()
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .selectable(false)
                .hover(|s| {
                    s.border_color(theme::fg_subtle())
                        .cursor(floem::style::CursorStyle::Text)
                })
        })
}

/// A square button with a glyph, as the note's foot steps with.
fn nav_button(
    icon: SidebarIcon,
    debug: &'static str,
    enabled: impl Fn() -> bool + 'static,
    action: impl Fn() + 'static,
) -> impl IntoView {
    icon_action(icon, debug, enabled, action).style(|s| {
        s.size(28.0, theme::THREAD_CONTROL - 4.0)
            .items_center()
            .justify_center()
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .flex_shrink(0.0_f32)
    })
}

/// "Note on lines R10 to R12", as a thread's header words its lines;
/// nothing when the lines aren't in Code's file.
fn lines_heading(anchor: Option<&Anchor>) -> impl IntoView {
    let words = anchor
        .map(|a| {
            let side = match a.side {
                Side::Left => "L",
                Side::Right => "R",
            };
            if a.start_line == a.line {
                format!("Note on line {side}{}", a.line)
            } else {
                format!("Note on lines {side}{} to {side}{}", a.start_line, a.line)
            }
        })
        .unwrap_or_default();
    Label::derived(move || words.clone()).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg_muted())
            .selectable(false)
            .flex_shrink(0.0_f32)
    })
}

/// What a Follow up on place `index` sends ahead of what is typed: the
/// note's words quoted, so the thread it starts says what it asks about.
pub fn follow_up_preface(state: ReviewState, (id, index): &(String, usize)) -> String {
    floem::reactive::Effect::untrack(|| kept(state))
        .iter()
        .find(|w| w.id == *id)
        .and_then(|w| w.steps.get(*index).map(quote))
        .unwrap_or_default()
}

/// The note's words quoted, for a follow-up about its lines to start
/// from.
fn quote(step: &Step) -> String {
    let said: Vec<&str> = step
        .body
        .iter()
        .filter_map(|part| match part {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if said.is_empty() {
        return String::new();
    }
    let quoted: Vec<String> = said
        .join("\n\n")
        .lines()
        .map(|line| match line {
            "" => ">".to_string(),
            line => format!("> {line}"),
        })
        .collect();
    format!("{}\n\n", quoted.join("\n"))
}

/// A body in order: prose, the lines it cites as links, the threads it
/// names and its diagrams.
fn body_view(state: ReviewState, body: &[Part]) -> Stack {
    let parts = body.iter().map(|part| match part {
        Part::Text { text } => crate::review::markdown::render_linked(text, state).into_any(),
        Part::Code(lines) => code_link(state, lines.clone()).into_any(),
        Part::Thread { id } => thread_row(state, id.clone()).into_any(),
        Part::Diagram { source } => crate::review::diagram::view(source.clone()).into_any(),
    });
    Stack::vertical_from_iter(parts.collect::<Vec<_>>())
        .style(|s| s.flex_col().width_full().min_width(0.0).gap(8.0))
}

/// Lines a note cites, as a link that opens them in Code: a line from
/// before the change at the head line it became, where Code's file has
/// it.
fn code_link(state: ReviewState, lines: Lines) -> impl IntoView {
    let words = match lines.end > lines.start {
        true => format!("{}\u{2013}{}", where_of(&lines), lines.end),
        false => where_of(&lines),
    };
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Link, theme::fg_subtle)
            .style(|s| s.size(12.0, 12.0).flex_shrink(0.0_f32)),
        Label::derived(move || words.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(theme::github())
                .text_ellipsis()
                .min_width(0.0)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| {
        let (start, end) = (lines.start as usize, lines.end as usize);
        let (first, last) = match lines.side {
            CodeSide::Head => (start, end),
            CodeSide::Base => state
                .files
                .with_untracked(|files| {
                    let file = files.get(&lines.path)?;
                    Some((head_line_of(file, start), head_line_of(file, end)))
                })
                .unwrap_or((start, end)),
        };
        state.one_move(|| {
            if state.tab.get_untracked() != ItemTab::Code {
                state.show_tab(ItemTab::Code);
            }
            // Through first: an editor already open takes the jump as it
            // is written.
            state.jump_through.set(Some(last.saturating_sub(1)));
            state.go_to_file(&lines.path, Some((first.saturating_sub(1), 0)));
        });
    })
    .debug_name("walkthrough-code-link")
    .style(|s| {
        clickable(s)
            .items_center()
            .gap(4.0)
            .min_width(0.0)
            .hover(|s| s.background(theme::canvas_hover()))
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
    Effect::new(move |_| {
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
/// walkthroughs arrive with the guidance the newest was written with.
/// Cmd-Enter writes. Hidden while one writes.
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
                "What should it dwell on? (optional)",
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

/// A thread the note names: where it is, who started it and how, opening
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
    use crate::backend::{Background, Chapter, Command, Hunk};
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

    /// Whether `id` shows: neither it nor a view over it is hidden.
    fn showing(id: floem::ViewId) -> bool {
        let mut at = Some(id);
        while let Some(id) = at {
            if id.is_hidden() {
                return false;
            }
            at = id.parent();
        }
        true
    }

    fn click_on(harness: &mut HeadlessHarness, id: floem::ViewId) {
        let center = id.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        settle(harness);
    }

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
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

    fn place(title: &str, chapter: usize, path: &str, line: u32, text: &str) -> Step {
        Step {
            title: title.into(),
            chapter,
            at: Lines {
                path: path.into(),
                side: CodeSide::Head,
                start: line,
                end: line,
            },
            text: vec![text.into()],
            body: Vec::new(),
            weigh: false,
            unplaced: false,
        }
    }

    /// A walkthrough of the branch at `head`: a chapter of two places and
    /// a follow-through of one.
    fn walkthrough(id: &str, head: &str) -> Walkthrough {
        let mut parse = place("Parse the request", 0, "src/x.rs", 2, "beta2");
        parse.weigh = true;
        parse.body = vec![
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
            Part::Thread { id: "gh-9".into() },
        ];
        let mut store = place("Store it", 0, "src/y.rs", 2, "fn caller() {}");
        store.body = vec![Part::Text {
            text: "Then it keeps it.".into(),
        }];
        Walkthrough {
            id: id.into(),
            scope: Scope::Branch,
            written_at: Some(jiff::Timestamp::now()),
            model: "Sonnet".into(),
            merge_base: "base-7".into(),
            head: head.into(),
            snapshot_ref: None,
            summary: "Stores each request **once**.".into(),
            background: Some(Background {
                title: "How requests are stored".into(),
                body: vec![Part::Text {
                    text: "Each request is kept once.".into(),
                }],
            }),
            chapters: vec![
                Chapter {
                    title: "The request".into(),
                    follow_through: false,
                },
                Chapter {
                    title: "Follow-through".into(),
                    follow_through: true,
                },
            ],
            steps: vec![parse, store, place("Rename", 1, "src/x.rs", 4, "delta")],
            hunks: vec![Hunk {
                id: "h1".into(),
                path: "src/x.rs".into(),
                side: CodeSide::Head,
                start: 1,
                end: 3,
            }],
            guidance: String::new(),
            read: BTreeSet::new(),
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

    fn with_files(state: ReviewState) {
        let (x, y) = files();
        state.files.update(|files| {
            files.insert("src/x.rs".into(), x);
            files.insert("src/y.rs".into(), y);
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

    /// The pane in the drawer, over a recording backend.
    fn pane_harness(
        state: ReviewState,
    ) -> (
        HeadlessHarness,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let root = TestRoot::new();
        let app = AppState::new();
        let (backend, commands) = Backend::recording();
        let view = pane(app, state, Rc::new(backend));
        let harness = HeadlessHarness::new_with_size(root, view, 360.0, 1200.0);
        (harness, commands)
    }

    /// The places' notes, one over the other, as Code would draw them
    /// under their lines.
    fn notes_harness(
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
        let harness = HeadlessHarness::new_with_size(root, view, 1000.0, 900.0);
        (harness, commands)
    }

    /// The pane asks for what is kept; with nothing kept it offers the
    /// form, writes for the scope Code shows with the agent picked and
    /// the reader's guidance, and offers Stop while it does.
    #[test]
    fn the_pane_asks_for_whats_kept_then_writes_and_can_stop() {
        let state = state_on_branch();
        let (mut harness, mut commands) = pane_harness(state);
        settle(&mut harness);
        assert!(commands_of(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::LoadWalkthrough { item_id } if item_id == "item-7")));

        apply(state, "item-7".into(), WalkthroughOutcome::Loaded(vec![]));
        settle(&mut harness);
        assert!(named(harness.root_id(), "walkthrough-form").is_some());
        assert!(find(harness.root_id(), "An agent reads the branch").is_some());
        assert!(!showing(
            named(harness.root_id(), "walkthrough-runs").unwrap()
        ));

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
            Command::WriteWalkthrough { item_id, scope, backend, model, guidance, .. }
                if item_id == "item-7"
                    && *scope == Scope::Branch
                    && *backend == perch_agent::Backend::Codex
                    && model == "gpt-5.5"
                    && guidance == "Start from the cache."
        )));
        assert!(find(harness.root_id(), "Writing a walkthrough of the branch").is_some());
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
        let stop = named(harness.root_id(), "walkthrough-stop").unwrap();
        click_on(&mut harness, stop);
        assert!(commands_of(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::StopWalkthrough { item_id } if item_id == "item-7")));

        apply(state, "item-7".into(), WalkthroughOutcome::Stopped);
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
            ..walkthrough("w1", "head-7")
        };
        apply(
            state,
            "a".into(),
            WalkthroughOutcome::Loaded(vec![kept("Start from the cache.")]),
        );
        state.walkthrough_guidance.update(|drafts| {
            drafts.insert("b".into(), "Mine.".into());
        });
        apply(
            state,
            "b".into(),
            WalkthroughOutcome::Written(Box::new(kept("Start from the cache."))),
        );
        let drafts = state.walkthrough_guidance.get();
        assert_eq!(
            drafts.get("a").map(String::as_str),
            Some("Start from the cache.")
        );
        assert_eq!(drafts.get("b").map(String::as_str), Some("Mine."));
    }

    /// Written, the pane shows the summary open and the background folded,
    /// how many places are read, and the chapters: the first open with its
    /// places, the one to weigh marked; the follow-through folded with its
    /// count. Start goes to the first place, as one move, and a place's
    /// row goes to it. Its check and Mark all keep places read.
    #[test]
    fn the_outline_lists_chapters_and_places_and_keeps_them_read() {
        let state = state_on_branch();
        with_files(state);
        let (mut harness, mut commands) = pane_harness(state);
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(Box::new(walkthrough("w1", "head-7"))),
        );
        settle(&mut harness);
        let root = harness.root_id();

        assert!(named(root, "walkthrough-outline").is_some());
        assert!(find(root, "Stores each request").is_some_and(showing));
        assert!(find(root, "How requests are stored").is_some_and(showing));
        assert!(!find(root, "Each request is kept once.").is_some_and(showing));
        assert!(find(root, "0 of 3 read").is_some());
        assert!(find(root, "1 The request").is_some());
        let parse = named(root, "walkthrough-place:1").unwrap();
        assert!(showing(parse));
        assert!(find(parse, "x.rs:2").is_some());
        assert!(!showing(named(root, "walkthrough-place:3").unwrap()));
        let follow = named(root, "walkthrough-chapter:2").unwrap();
        click_on(&mut harness, follow);
        assert!(showing(named(root, "walkthrough-place:3").unwrap()));

        let start = named(root, "walkthrough-start").unwrap();
        click_on(&mut harness, start);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(0));
        assert_eq!(state.tab.get_untracked(), ItemTab::Code);
        assert_eq!(
            state.pending_jump.get_untracked(),
            Some(("src/x.rs".to_string(), 1, 0))
        );
        assert!(state.history.with_untracked(History::can_go_back));
        assert!(!showing(start));

        let store = named(root, "walkthrough-place:2").unwrap();
        click_on(&mut harness, store);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(1));

        // The row's check is its last part.
        let check = *store.children().last().unwrap();
        click_on(&mut harness, check);
        assert!(commands_of(&mut commands).iter().any(|c| matches!(
            c,
            Command::MarkWalkthroughRead { item_id, id, places, read: true }
                if item_id == "item-7" && id == "w1" && places == &[1]
        )));
        assert!(find(root, "1 of 3 read").is_some());
        let all = named(root, "walkthrough-mark-all").unwrap();
        click_on(&mut harness, all);
        assert!(commands_of(&mut commands).iter().any(|c| matches!(
            c,
            Command::MarkWalkthroughRead { places, read: true, .. } if places == &[0, 2]
        )));
        assert!(find(root, "3 of 3 read").is_some());
        assert!(find(root, "Clear all").is_some());
    }

    /// The pane shows the scope's own walkthroughs: the newest, or one
    /// picked from the list. A scope with none offers the form and the
    /// branch's; Staged can't have one.
    #[test]
    fn walkthroughs_are_of_the_scope_code_shows() {
        let state = state_on_branch();
        let (mut harness, _commands) = pane_harness(state);
        let commit = Walkthrough {
            scope: Scope::Commit {
                sha: "c0ffee".into(),
            },
            summary: "Only the commit.".into(),
            ..walkthrough("w3", "head-7")
        };
        let older = Walkthrough {
            summary: "An older one.".into(),
            ..walkthrough("w1", "head-7")
        };
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Loaded(vec![commit, walkthrough("w2", "head-7"), older]),
        );
        settle(&mut harness);
        let root = harness.root_id();
        assert!(find(root, "Stores each request").is_some_and(showing));
        assert!(showing(named(root, "walkthrough-runs").unwrap()));

        state.walkthrough_picked.update(|picked| {
            picked.insert("item-7".into(), "w1".into());
        });
        settle(&mut harness);
        assert!(find(root, "An older one.").is_some_and(showing));

        state.scope.set(Scope::Commit {
            sha: "c0ffee".into(),
        });
        settle(&mut harness);
        assert!(find(root, "Only the commit.").is_some_and(showing));

        state.scope.set(Scope::Commit { sha: "dead".into() });
        settle(&mut harness);
        assert!(named(root, "walkthrough-form").is_some());
        assert!(find(root, "The branch has a walkthrough").is_some_and(showing));
        assert!(named(root, "walkthrough-write").is_some_and(showing));

        state.scope.set(Scope::Staged);
        settle(&mut harness);
        assert!(find(root, "Pick Uncommitted").is_some_and(showing));
        assert!(!named(root, "walkthrough-write").is_some_and(showing));
    }

    /// One written for an older head says so.
    #[test]
    fn a_walkthrough_of_an_older_head_says_so() {
        let state = state_on_branch();
        let (mut harness, _commands) = pane_harness(state);
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Loaded(vec![walkthrough("w1", "old-head")]),
        );
        settle(&mut harness);
        assert!(find(harness.root_id(), "Written for old-hea").is_some_and(showing));
        assert!(find(harness.root_id(), "an older head").is_some());
    }

    /// A place's note is drawn as a thread of Perch's own: its lines, its
    /// chapter, Worth a close look, which place of how many, then its
    /// body in order, the lines it cites as a link and the thread it
    /// names, which opens on Conversation. Next marks it read and goes
    /// on; there is no back from the first. Follow up opens the composer
    /// in its place, as a thread's does, empty, and the way on waits; what
    /// is sent asks about its lines with the note quoted.
    #[test]
    fn a_note_draws_its_body_and_goes_on() {
        let state = state_on_branch();
        with_files(state);
        state.comment_threads.set(Arc::new(vec![thread_on_x()]));
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(Box::new(walkthrough("w1", "head-7"))),
        );
        state.walkthrough_at.set(Some(0));
        let (mut harness, mut commands) = notes_harness(state);
        settle(&mut harness);
        let root = harness.root_id();

        assert!(find(root, "Note on line R2").is_some());
        assert!(find(root, "1 \u{b7} The request").is_some());
        assert!(find(root, "Worth a close look").is_some_and(showing));
        assert!(find(root, "Perch only").is_some());
        let top = |text: &str| {
            find(root, text)
                .unwrap_or_else(|| panic!("no {text}"))
                .get_visual_rect_no_clip()
                .y0
        };
        let order = [
            top("1 of 3"),
            top("First it reads the request."),
            top("x.rs:1\u{2013}3"),
            top("Then the line after."),
            top("Thread on src/x.rs:2"),
            top("2 of 3"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");

        let link = named(root, "walkthrough-code-link").unwrap();
        click_on(&mut harness, link);
        assert_eq!(
            state.pending_jump.get_untracked(),
            Some(("src/x.rs".to_string(), 0, 0))
        );
        assert_eq!(state.jump_through.get_untracked(), Some(2));

        let thread = find(root, "Thread on src/x.rs:2 \u{b7} ana: Why b2?").unwrap();
        click_on(&mut harness, thread);
        assert_eq!(state.tab.get_untracked(), ItemTab::Conversation);
        assert_eq!(
            state.focus_card.get_untracked(),
            Some(CardId::Thread("gh-9".into()))
        );

        let follow = named(root, "walkthrough-follow-up").unwrap();
        click_on(&mut harness, follow);
        let composer = state.composer.get_untracked().expect("a composer");
        assert_eq!(
            (
                composer.thread_id.clone(),
                composer.step.as_ref().map(|(_, at)| *at)
            ),
            (None, Some(0))
        );
        let anchor = composer.anchor.clone().unwrap();
        assert_eq!((anchor.path.as_str(), anchor.line), ("src/x.rs", 2));
        assert_eq!(composer.draft, "");
        let step = named(root, "walkthrough-step").unwrap();
        assert!(named(step, "composer-box").is_some_and(showing));
        assert!(named(step, "walkthrough-follow-up").is_none());
        assert!(!showing(named(step, "walkthrough-note-next").unwrap()));
        // Send starts the thread with the note quoted over what was typed.
        state.composer_draft.set("Why?".into());
        let composing_root = TestRoot::new();
        let (backend, mut sent) = Backend::recording();
        let view = threads::composer_view(
            AppState::new(),
            state,
            Rc::new(backend),
            composer,
            ThreadFace::Inline,
        );
        let mut composing = HeadlessHarness::new_with_size(composing_root, view, 600.0, 300.0);
        settle(&mut composing);
        let send = named(composing.root_id(), "composer-send").unwrap();
        click_on(&mut composing, send);
        let asked = commands_of(&mut sent).into_iter().find_map(|c| match c {
            Command::Ask { question, .. } => Some(question),
            _ => None,
        });
        let asked = asked.expect("the question went");
        assert!(
            asked.starts_with("> First it reads the request."),
            "{asked}"
        );
        assert!(asked.ends_with("\n\nWhy?"), "{asked}");
        assert!(state.composer.get_untracked().is_none());
        settle(&mut harness);
        assert!(showing(named(step, "walkthrough-note-next").unwrap()));

        let next = named(root, "walkthrough-note-next").unwrap();
        click_on(&mut harness, next);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(1));
        assert_eq!(
            state.pending_jump.get_untracked(),
            Some(("src/y.rs".to_string(), 1, 0))
        );
        assert!(commands_of(&mut commands).iter().any(|c| matches!(
            c,
            Command::MarkWalkthroughRead { places, read: true, .. } if places == &[0]
        )));
        assert_eq!(shown(state).unwrap().read, [0].into());

        // No place before the first.
        let back = named(root, "walkthrough-note-back").unwrap();
        state.walkthrough_at.set(Some(0));
        click_on(&mut harness, back);
        assert_eq!(state.walkthrough_at.get_untracked(), Some(0));
    }

    /// A base line is the head line it became: moved down by what was put
    /// in above it, and a removed one at the line that follows it. A file
    /// the change leaves alone is the same on both sides.
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

    /// A place's lines are where it says while they are still there; in a
    /// file that has moved on, the same lines nearest to there; and
    /// nowhere once they are gone. Removed lines are found on the base,
    /// and an unchanged file's base is its head.
    #[test]
    fn a_place_finds_its_lines_in_a_file_that_moved() {
        let (x, y) = files();
        let step = &walkthrough("w1", "head-7").steps[0];
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

    /// While the pane is out, each file has a note for each place in it,
    /// right under its lines and over a thread's there; with it in, none.
    #[test]
    fn places_have_notes_in_code_while_the_pane_is_out() {
        let _root = TestRoot::new();
        let state = state_on_branch();
        with_files(state);
        state.comment_threads.set(Arc::new(vec![thread_on_x()]));
        apply(
            state,
            "item-7".into(),
            WalkthroughOutcome::Written(Box::new(walkthrough("w1", "head-7"))),
        );
        let ids = |path: &str| -> Vec<CardId> {
            state.cards_in(path).into_iter().map(|c| c.id).collect()
        };
        assert_eq!(ids("src/x.rs"), vec![CardId::Thread("gh-9".into())]);
        assert!(ids("src/y.rs").is_empty());

        state.walkthrough_out.set(true);
        assert_eq!(
            ids("src/x.rs"),
            vec![
                CardId::Step(0),
                CardId::Step(2),
                CardId::Thread("gh-9".into())
            ]
        );
        assert_eq!(ids("src/y.rs"), vec![CardId::Step(1)]);
        assert!(state.anchors_in("src/y.rs").contains(&(1, 1)));
        let card = state.cards_in("src/x.rs").remove(0);
        assert_eq!(card.side, Side::Right);
        assert!(card.rows >= 7);

        state.walkthrough_out.set(false);
        assert_eq!(ids("src/x.rs"), vec![CardId::Thread("gh-9".into())]);
    }
}
