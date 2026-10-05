//! A run as a thread shows it. While it goes: a line saying what the
//! agent is doing and for how long, with Stop at its end; the agent's
//! words as prose, where it said them; and its tool calls between them as
//! rows in an inset box, an icon for each kind of call and the file one
//! names as a chip that opens it. The latest box shows only its last few
//! rows, and one the agent has talked past folds to a count. Once the run
//! is over, all of it folds to how long it worked and what it did.
//!
//! A step is only a line of text, "Read src/x.rs", so its kind, its verb
//! and what it names are read back from the words the agent crate wrote.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashSet;
use std::f64::consts::TAU;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use floem::context::PaintCx;
use floem::kurbo::{Circle, Point, Stroke, Vec2};
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack};
use floem::{View, ViewId};
use perch_github::{RunRecord, RunStatus, RunStep};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards;
use crate::review::code_refs::{written_place, Refs};
use crate::review::model::ReviewState;
use crate::review::threads::{steps_summary, worked_for};
use crate::review::timeline::count_of;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_button, sidebar_glyph};

/// How many of the latest box's rows show while the run goes.
const TAIL: usize = 4;

// ---------------------------------------------------------------------
// What a step says
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Read,
    Search,
    List,
    Run,
    Edit,
    Delegate,
    Other,
}

/// A step's line taken apart: the verb as written, as it reads once the
/// call is done and while it is going, what the call was on, and for an
/// edit the lines it adds and removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepLine {
    pub kind: StepKind,
    pub verb: String,
    pub done: String,
    pub doing: String,
    pub arg: String,
    /// Whether `arg` is a path, which may open.
    pub path: bool,
    pub counts: Option<(usize, usize)>,
    /// A call a delegated agent made, which the agent crate marks `↳`.
    pub nested: bool,
}

/// The line's start, its kind, how it reads done and doing, and whether
/// what follows is a path. The first that fits is taken, so a longer
/// start goes before a shorter one it begins with.
const VERBS: &[(&str, StepKind, &str, &str, bool)] = &[
    ("Read ", StepKind::Read, "Read", "Reading", true),
    (
        "Searched for ",
        StepKind::Search,
        "Searched for",
        "Searching for",
        false,
    ),
    ("Listed ", StepKind::List, "Listed", "Listing", true),
    (
        "Ran outside the sandbox: ",
        StepKind::Run,
        "Ran outside the sandbox",
        "Running outside the sandbox",
        false,
    ),
    ("Ran ", StepKind::Run, "Ran", "Running", false),
    ("Asked to edit ", StepKind::Edit, "Edited", "Editing", true),
    ("Edited ", StepKind::Edit, "Edited", "Editing", true),
    (
        "Delegated: ",
        StepKind::Delegate,
        "Delegated",
        "Delegating",
        false,
    ),
];

pub fn step_line(summary: &str) -> StepLine {
    let (nested, text) = match summary.strip_prefix("↳ ") {
        Some(rest) => (true, rest),
        None => (false, summary),
    };
    for &(prefix, kind, done, doing, path) in VERBS {
        let Some(arg) = text.strip_prefix(prefix) else {
            continue;
        };
        let (arg, counts) = if kind == StepKind::Edit {
            split_counts(arg)
        } else {
            (arg.to_string(), None)
        };
        return StepLine {
            kind,
            verb: prefix.trim_end().trim_end_matches(':').to_string(),
            done: done.to_string(),
            doing: doing.to_string(),
            arg,
            path,
            counts,
            nested,
        };
    }
    StepLine {
        kind: StepKind::Other,
        verb: text.to_string(),
        done: text.to_string(),
        doing: text.to_string(),
        arg: String::new(),
        path: false,
        counts: None,
        nested,
    }
}

/// `a.rs (+12 −3)` as `a.rs` and the counts, with whatever followed them,
/// the error a failed edit adds, kept.
fn split_counts(text: &str) -> (String, Option<(usize, usize)>) {
    let parsed = text.find(" (+").and_then(|start| {
        let rest = &text[start + 3..];
        let end = rest.find(')')?;
        let (added, removed) = rest[..end].split_once(" \u{2212}")?;
        let counts = (added.parse().ok()?, removed.parse().ok()?);
        Some((format!("{}{}", &text[..start], &rest[end + 1..]), counts))
    });
    match parsed {
        Some((text, counts)) => (text, Some(counts)),
        None => (text.to_string(), None),
    }
}

fn icon_of(kind: StepKind) -> SidebarIcon {
    match kind {
        StepKind::Read => SidebarIcon::File,
        StepKind::Search => SidebarIcon::Search,
        StepKind::List => SidebarIcon::Folder,
        StepKind::Run => SidebarIcon::Terminal,
        StepKind::Edit => SidebarIcon::Pencil,
        StepKind::Delegate => SidebarIcon::Agents,
        StepKind::Other => SidebarIcon::Source,
    }
}

/// Whether what a step names reads as code: a path, a command, a pattern.
fn is_code(kind: StepKind) -> bool {
    !matches!(kind, StepKind::Delegate | StepKind::Other)
}

/// What the run is doing now, as its line says it: the verb, what it is
/// on, and whether that is code. The latest call still waiting on its
/// result, or thinking between calls.
pub fn doing_now(run: &RunRecord) -> (String, String, bool) {
    let word = |w: &str| (w.to_string(), String::new(), false);
    match run.status {
        RunStatus::Queued => word("Queued"),
        RunStatus::Starting => word("Starting"),
        RunStatus::Stopping => word("Stopping"),
        _ => match run
            .steps
            .iter()
            .rev()
            .find(|s| s.pending && !s.note && !s.user)
        {
            Some(step) => {
                let line = step_line(&step.summary);
                (line.doing, line.arg, is_code(line.kind))
            }
            None => word("Thinking"),
        },
    }
}

/// How long a run has gone, from when it started, or from when it was
/// asked for while it waits.
fn elapsed(run: &RunRecord, now: jiff::Timestamp) -> String {
    let from = run.started_at.unwrap_or(run.queued_at);
    let ms = now.duration_since(from).as_millis().max(0);
    worked_for(u64::try_from(ms).unwrap_or(0))
}

/// "18.2k tokens", with what it cost when that is known.
fn usage_label(run: &RunRecord) -> Option<String> {
    let usage = run.usage.as_ref()?;
    let tokens = usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0);
    if tokens == 0 {
        return None;
    }
    let mut label = match tokens {
        0..1_000 => format!("{tokens} tokens"),
        1_000..1_000_000 => format!("{:.1}k tokens", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M tokens", tokens as f64 / 1_000_000.0),
    };
    if let Some(cost) = run.cost_usd {
        label.push_str(&format!(" \u{00b7} ${cost:.2}"));
    }
    Some(label)
}

// ---------------------------------------------------------------------
// The spinner
// ---------------------------------------------------------------------

/// Seconds a spinner takes to turn once.
const TURN: f64 = 0.9;
/// How often a spinner on the screen is drawn again.
const FRAME: Duration = Duration::from_millis(66);

/// A quarter arc going round a faint ring, sized by its style. It is
/// drawn, not an SVG, since vger draws an SVG unturned; each paint asks
/// for the next, so a spinner off the screen or gone costs nothing.
pub struct Spinner {
    id: ViewId,
    color: Box<dyn Fn() -> Color>,
    armed: Rc<Cell<bool>>,
}

pub fn spinner(color: impl theme::Ink) -> Spinner {
    Spinner {
        id: ViewId::new(),
        color: Box::new(color),
        armed: Rc::new(Cell::new(false)),
    }
}

impl View for Spinner {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> Cow<'static, str> {
        "spinner".into()
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let Some(layout) = self.id.get_layout() else {
            return;
        };
        let (width, height) = (f64::from(layout.size.width), f64::from(layout.size.height));
        let stroke = 1.5;
        let radius = width.min(height) / 2.0 - stroke / 2.0;
        if radius <= 0.0 {
            return;
        }
        let center = Point::new(width / 2.0, height / 2.0);
        cx.stroke(
            &Circle::new(center, radius),
            theme::border(),
            &Stroke::new(stroke),
        );
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let arc = floem::kurbo::Arc {
            center,
            radii: Vec2::new(radius, radius),
            start_angle: (seconds / TURN).fract() * TAU,
            sweep_angle: TAU / 4.0,
            x_rotation: 0.0,
        };
        cx.stroke(&arc, (self.color)(), &Stroke::new(stroke));
        if !self.armed.replace(true) {
            let (id, armed) = (self.id, self.armed.clone());
            floem::action::exec_after(FRAME, move |_| {
                armed.set(false);
                if id.is_valid() {
                    id.request_paint();
                }
            });
        }
    }
}

/// A spinner in a thread's header while an agent works in the thread,
/// so a folded thread says so too.
pub fn running_mark(state: ReviewState, thread_id: String) -> impl IntoView {
    let running = Memo::new(move |_| {
        state.comment_threads.with(|threads| {
            threads
                .iter()
                .any(|t| t.id == thread_id && t.running_run().is_some())
        })
    });
    dyn_container(
        move || running.get(),
        |running| {
            if !running {
                return Empty::new().into_any();
            }
            cards::tip(
                spinner(theme::fg_muted).style(|s| s.size(10.0, 10.0).margin_left(8.0)),
                "An agent is working in this thread".to_string(),
            )
            .into_any()
        },
    )
    .debug_name("thread-running")
    .style(|s| s.items_center().flex_shrink(0.0_f32))
}

// ---------------------------------------------------------------------
// The line over a running run
// ---------------------------------------------------------------------

/// What the run is doing and for how long, the tokens it has used under
/// the pointer, and Stop, or Cancel while it waits for its turn. The
/// labels follow the run, so the line is built once for it.
pub fn run_header(app: AppState, backend: Rc<Backend>, live: Memo<RunRecord>) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let doing = Memo::new(move |_| live.with(doing_now));
    let status = Memo::new(move |_| live.with(|run| run.status));
    let usage = Memo::new(move |_| live.with(usage_label));
    let run_id = live.with_untracked(|run| run.id.clone());
    let stop = cards::tip_with(
        icon_button(
            || SidebarIcon::Stop,
            theme::fg_muted,
            move || backend.cancel_run(&run_id),
        )
        .debug_name("run-stop"),
        move || {
            if status.get_untracked() == RunStatus::Queued {
                "Cancel".to_string()
            } else {
                "Stop".to_string()
            }
        },
    );
    let row = Stack::horizontal((
        spinner(theme::fg_muted)
            .style(|s| s.size(12.0, 12.0).flex_shrink(0.0_f32).margin_right(8.0)),
        Label::derived(move || doing.get().0).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg())
                .selectable(false)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || doing.get().1).style(move |s| {
            let (_, arg, code) = doing.get();
            s.font_size(if code {
                theme::TEXT_BADGE
            } else {
                theme::TEXT_META
            })
            .apply_if(code, |s| s.font_family(theme::CODE_FONT_STACK.to_string()))
            .color(theme::fg_muted())
            .margin_left(5.0)
            .text_ellipsis()
            .min_width(0.0)
            .apply_if(arg.is_empty(), |s| s.hide())
        }),
        Label::derived(move || {
            let now = app.now.get();
            format!("\u{00b7} {}", live.with(|run| elapsed(run, now)))
        })
        .debug_name("run-elapsed")
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .margin_left(6.0)
                .selectable(false)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || format!("\u{00b7} {}", usage.get().unwrap_or_default())).style(
            move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_subtle())
                    .margin_left(6.0)
                    .selectable(false)
                    .flex_shrink(0.0_f32)
                    .apply_if(!hovered.get() || usage.get().is_none(), |s| s.hide())
            },
        ),
        Empty::new().style(|s| s.flex_grow(1.0_f32).min_width(8.0)),
        Container::new(stop).style(move |s| {
            s.flex_shrink(0.0_f32)
                .apply_if(status.get() == RunStatus::Stopping, |s| s.hide())
        }),
    ))
    .debug_name("run-header")
    .style(|s| s.items_center().width_full().height(28.0));
    cards::track_hover(row, hovered)
}

// ---------------------------------------------------------------------
// The steps and words
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Segment {
    Note(String),
    Tools(Vec<RunStep>),
    /// What the reader said while it worked, and whether it is unread.
    Steer(String, bool),
}

/// The steps as the agent went: each thing it said, each run of calls
/// between, and what the reader told it.
fn segments(steps: &[RunStep]) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for step in steps {
        if step.user {
            out.push(Segment::Steer(step.summary.clone(), step.pending));
        } else if step.note {
            out.push(Segment::Note(step.summary.clone()));
        } else if let Some(Segment::Tools(calls)) = out.last_mut() {
            calls.push(step.clone());
        } else {
            out.push(Segment::Tools(vec![step.clone()]));
        }
    }
    out
}

/// Where a run of calls sits: the latest while the run goes, one before
/// it, or one of a run that is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Place {
    Live,
    Past,
    Done,
}

/// The agent's words and its calls, in the order it went. While it runs
/// the latest box shows its last rows and the ones before fold to a
/// count; once it is over every box is whole. What the reader opens is
/// kept across events, which build only the part that changed.
pub fn timeline(
    state: ReviewState,
    steps: impl Fn() -> Vec<RunStep> + 'static,
    running: impl Fn() -> bool + 'static,
) -> impl IntoView {
    let opened: RwSignal<HashSet<usize>> = RwSignal::new(HashSet::new());
    let whole = RwSignal::new(false);
    let items = Memo::new(move |_| {
        let segments = segments(&steps());
        let running = running();
        let latest = segments
            .iter()
            .rposition(|s| matches!(s, Segment::Tools(_)));
        segments
            .into_iter()
            .enumerate()
            .map(|(i, segment)| {
                let place = match running {
                    false => Place::Done,
                    true if Some(i) == latest => Place::Live,
                    true => Place::Past,
                };
                (i, segment, place)
            })
            .collect::<Vec<_>>()
    });
    dyn_stack(
        move || items.get(),
        |item| item.clone(),
        move |(i, segment, place)| match segment {
            Segment::Note(text) => note_view(state, &text),
            Segment::Steer(text, unread) => steer_view(state, &text, unread, place),
            Segment::Tools(calls) => match place {
                Place::Live => live_box(state, calls, whole),
                Place::Past => folded_box(state, i, calls, opened),
                Place::Done => tools_box(rows(state, &calls, false)),
            },
        },
    )
    .debug_name("run-timeline")
    .style(|s| s.flex_col().width_full().gap(8.0))
}

fn note_view(state: ReviewState, text: &str) -> floem::AnyView {
    Container::new(crate::review::markdown::render_linked(text, state))
        .debug_name("run-note")
        .style(|s| s.width_full().min_width(0.0))
        .into_any()
}

/// What the reader told the agent, set off by a rule at its left. One
/// not yet read says so: waiting while the run goes, never once it is
/// over.
fn steer_view(state: ReviewState, text: &str, unread: bool, place: Place) -> floem::AnyView {
    let caption = match (unread, place) {
        (false, _) => None,
        (true, Place::Done) => Some("The agent ended without reading this"),
        (true, _) => Some("The agent reads this after its current step"),
    };
    Stack::vertical((
        Label::derived(|| "You".to_string()).style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_muted())
                .selectable(false)
        }),
        Container::new(crate::review::markdown::render_linked(text, state))
            .style(|s| s.width_full().min_width(0.0)),
        match caption {
            Some(caption) => Label::derived(move || caption.to_string())
                .debug_name("run-steer-unread")
                .style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_subtle())
                        .selectable(false)
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
    ))
    .debug_name("run-steer")
    .style(|s| {
        s.flex_col()
            .width_full()
            .min_width(0.0)
            .gap(2.0)
            .padding_left(10.0)
            .border_left(2.0)
            .border_color(theme::border())
    })
    .into_any()
}

/// The inset a run of calls sits in.
fn tools_box(inner: impl IntoView + 'static) -> floem::AnyView {
    Container::new(inner)
        .debug_name("run-steps")
        .style(|s| {
            s.width_full()
                .min_width(0.0)
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .border_radius(theme::RADIUS)
                .background(theme::canvas_subtle())
        })
        .into_any()
}

fn rows(state: ReviewState, calls: &[RunStep], live: bool) -> impl IntoView {
    Stack::vertical_from_iter(calls.iter().map(|call| step_row(state, call.clone(), live)))
        .style(|s| s.flex_col().width_full().min_width(0.0))
}

/// The latest run of calls: its last `TAIL` rows, and a line for the
/// ones before that shows them all.
fn live_box(state: ReviewState, calls: Vec<RunStep>, whole: RwSignal<bool>) -> floem::AnyView {
    let calls = Rc::new(calls);
    tools_box(dyn_container(
        move || whole.get(),
        move |all| {
            let hidden = if all {
                0
            } else {
                calls.len().saturating_sub(TAIL)
            };
            let earlier = if hidden == 0 {
                Empty::new().into_any()
            } else {
                fold_row(
                    count_of(hidden, "earlier step", "earlier steps"),
                    false,
                    move || whole.set(true),
                )
                .into_any()
            };
            Stack::vertical((earlier, rows(state, &calls[hidden..], true)))
                .style(|s| s.flex_col().width_full())
                .into_any()
        },
    ))
}

/// A run of calls the agent has talked past, folded to what it did until
/// it is clicked.
fn folded_box(
    state: ReviewState,
    index: usize,
    calls: Vec<RunStep>,
    opened: RwSignal<HashSet<usize>>,
) -> floem::AnyView {
    let calls = Rc::new(calls);
    let summary = steps_summary(&calls);
    let open = Memo::new(move |_| opened.with(|o| o.contains(&index)));
    tools_box(dyn_container(
        move || open.get(),
        move |open| {
            let line = fold_row(summary.clone(), open, move || {
                opened.update(|o| {
                    if !o.remove(&index) {
                        o.insert(index);
                    }
                })
            });
            if open {
                Stack::vertical((line, rows(state, &calls, true)))
                    .style(|s| s.flex_col().width_full())
                    .into_any()
            } else {
                line.into_any()
            }
        },
    ))
}

/// A chevron and a muted line that opens or closes what is under it.
fn fold_row(text: String, open: bool, toggle: impl Fn() + 'static) -> impl IntoView {
    Stack::horizontal((
        Container::new(
            sidebar_glyph(
                if open {
                    SidebarIcon::ChevronDown
                } else {
                    SidebarIcon::ChevronRight
                },
                theme::fg_subtle,
            )
            .style(|s| s.size(10.0, 10.0)),
        )
        .style(|s| {
            s.width(16.0)
                .items_center()
                .justify_center()
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || text.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .margin_left(6.0)
                .selectable(false)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| toggle())
    .debug_name("run-fold")
    .style(|s| clickable(s).items_center().width_full().height(22.0))
}

/// One call: its icon, or a spinner while it goes; its verb; what it was
/// on, a file as a chip that opens it; and what an edit added and
/// removed. A call that failed is red, with what went wrong after it.
fn step_row(state: ReviewState, step: RunStep, live: bool) -> floem::AnyView {
    let line = step_line(&step.summary);
    let pending = live && step.pending;
    let failed = !step.ok;
    let tint = if failed {
        theme::closed
    } else {
        theme::fg_muted
    };
    let icon = if pending {
        spinner(theme::fg_muted)
            .style(|s| s.size(11.0, 11.0))
            .into_any()
    } else if failed {
        sidebar_glyph(SidebarIcon::Warning, theme::closed)
            .style(|s| s.size(13.0, 13.0))
            .into_any()
    } else {
        sidebar_glyph(icon_of(line.kind), theme::fg_subtle)
            .style(|s| s.size(13.0, 13.0))
            .into_any()
    };
    let verb = if pending {
        line.doing.clone()
    } else if failed {
        line.verb.clone()
    } else {
        line.done.clone()
    };
    let arg = if line.arg.is_empty() {
        Empty::new().into_any()
    } else if line.path && !failed {
        path_chip(state, line.arg.clone())
    } else {
        let (arg, code) = (line.arg.clone(), is_code(line.kind));
        Label::derived(move || arg.clone())
            .style(move |s| {
                s.font_size(if code {
                    theme::TEXT_BADGE
                } else {
                    theme::TEXT_META
                })
                .apply_if(code, |s| s.font_family(theme::CODE_FONT_STACK.to_string()))
                .color(tint())
                .text_ellipsis()
                .min_width(0.0)
            })
            .into_any()
    };
    let counts = match line.counts {
        Some((added, removed)) => Stack::horizontal((
            Label::derived(move || format!("+{added}")).style(|s| {
                s.font_family(theme::CODE_FONT_STACK.to_string())
                    .font_size(theme::TEXT_BADGE)
                    .color(theme::status_added())
            }),
            Label::derived(move || format!("\u{2212}{removed}")).style(|s| {
                s.font_family(theme::CODE_FONT_STACK.to_string())
                    .font_size(theme::TEXT_BADGE)
                    .color(theme::status_deleted())
            }),
        ))
        .style(|s| s.gap(4.0).margin_left(6.0).flex_shrink(0.0_f32))
        .into_any(),
        None => Empty::new().into_any(),
    };
    let nested = line.nested;
    let alone = line.arg.is_empty();
    Stack::horizontal((
        Container::new(icon).style(|s| {
            s.width(16.0)
                .height(16.0)
                .items_center()
                .justify_center()
                .flex_shrink(0.0_f32)
        }),
        // A step no verb matched, often an error, is all verb: it gives
        // way at the edge instead of the missing argument.
        Label::derived(move || verb.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if pending { theme::fg() } else { tint() })
                .margin_left(6.0)
                .apply_if(alone, |s| s.text_ellipsis().min_width(0.0))
                .apply_if(!alone, |s| s.flex_shrink(0.0_f32))
        }),
        Container::new(arg).style(|s| s.margin_left(6.0).min_width(0.0).flex_shrink(1.0_f32)),
        counts,
    ))
    .debug_name("run-step")
    .style(move |s| {
        s.items_center()
            .width_full()
            .min_width(0.0)
            .height(24.0)
            .apply_if(nested, |s| s.padding_left(18.0))
    })
    .into_any()
}

/// A file a step names, as a chip that opens it in Files when the
/// worktree has it, or as plain code when it does not.
fn path_chip(state: ReviewState, path: String) -> floem::AnyView {
    let link = Refs::of(state).and_then(|refs| refs.file(&written_place(&path)?));
    let label = Label::derived(move || path.clone());
    match link {
        Some(link) => label
            .on_event_stop(el::Click, move |_, _| state.go_to_file(&link.path, None))
            .debug_name("step-file")
            .style(|s| {
                clickable(s)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .font_size(theme::TEXT_BADGE)
                    .color(theme::fg())
                    .padding_horiz(5.0)
                    .padding_vert(1.0)
                    .background(theme::canvas())
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(4.0)
                    .text_ellipsis()
                    .min_width(0.0)
                    .hover(|s| s.border_color(theme::fg_subtle()))
            })
            .into_any(),
        None => label
            .style(|s| {
                s.font_family(theme::CODE_FONT_STACK.to_string())
                    .font_size(theme::TEXT_BADGE)
                    .color(theme::fg_muted())
                    .text_ellipsis()
                    .min_width(0.0)
            })
            .into_any(),
    }
}

// ---------------------------------------------------------------------
// A run that is over
// ---------------------------------------------------------------------

/// The line over a run that is over, "Worked for 4m 12s · read 11 files,
/// ran 4 commands", which opens its words and calls beneath it.
pub fn run_fold(state: ReviewState, run: &RunRecord, mut steps: Vec<RunStep>) -> impl IntoView {
    // A run that finished with the reader's words unread asked them as
    // the next question, which the thread shows.
    if run.status == RunStatus::Completed {
        steps.retain(|step| !(step.user && step.pending));
    }
    let run_id = run.id.clone();
    let open = Memo::new(move |_| state.expanded_runs.with(|e| e.contains(&run_id)));
    let (main, detail) = fold_words(run, &steps);
    let toggle_id = run.id.clone();
    let steps = Rc::new(steps);
    Stack::vertical((
        Stack::horizontal((
            Container::new(dyn_container(
                move || open.get(),
                |open| {
                    sidebar_glyph(
                        if open {
                            SidebarIcon::ChevronDown
                        } else {
                            SidebarIcon::ChevronRight
                        },
                        theme::fg_subtle,
                    )
                    .style(|s| s.size(10.0, 10.0))
                    .into_any()
                },
            ))
            // Flush with the prose, and as wide as the running line's
            // spinner, so the words start where they did.
            .style(|s| s.width(12.0).items_center().flex_shrink(0.0_f32)),
            Label::derived(move || main.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .margin_left(8.0)
                    .selectable(false)
                    .flex_shrink(0.0_f32)
            }),
            Label::derived(move || detail.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .margin_left(5.0)
                    .selectable(false)
                    .text_ellipsis()
                    .min_width(0.0)
            }),
        ))
        .on_event_stop(el::Click, move |_, _| {
            state.expanded_runs.update(|e| {
                if !e.remove(&toggle_id) {
                    e.insert(toggle_id.clone());
                }
            })
        })
        .debug_name("run-fold")
        .style(|s| clickable(s).items_center().width_full().height(24.0)),
        dyn_container(
            move || open.get(),
            move |open| {
                if !open {
                    return Empty::new().into_any();
                }
                let steps = steps.clone();
                Container::new(timeline(state, move || steps.as_ref().clone(), || false))
                    .style(|s| s.width_full().padding_top(6.0))
                    .into_any()
            },
        )
        .style(|s| s.width_full()),
    ))
    .style(|s| s.flex_col().width_full())
}

/// The fold's words: how long the run worked, and, after a dot, what it
/// did. Only what it did when how long is not known.
fn fold_words(run: &RunRecord, steps: &[RunStep]) -> (String, String) {
    let main = crate::review::threads::run_summary(run);
    let did = steps_summary(steps);
    if !main.starts_with("Worked for") || did == "No steps" {
        return (main, String::new());
    }
    let mut did = did;
    if let Some(first) = did.get_mut(0..1) {
        first.make_ascii_lowercase();
    }
    (main, format!("\u{00b7} {did}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_line_is_read_back_into_its_parts() {
        let read = step_line("Read src/db.rs");
        assert_eq!(
            (read.kind, read.done.as_str(), read.doing.as_str()),
            (StepKind::Read, "Read", "Reading")
        );
        assert_eq!(read.arg, "src/db.rs");
        assert!(read.path);

        let search = step_line("Searched for fn open in src");
        assert_eq!(
            (search.kind, search.doing.as_str()),
            (StepKind::Search, "Searching for")
        );
        assert_eq!(search.arg, "fn open in src");

        let outside = step_line("Ran outside the sandbox: git commit -m x");
        assert_eq!(
            (outside.kind, outside.done.as_str()),
            (StepKind::Run, "Ran outside the sandbox")
        );
        assert_eq!(outside.arg, "git commit -m x");

        let nested = step_line("↳ Ran cargo test");
        assert!(nested.nested);
        assert_eq!(nested.arg, "cargo test");

        let other = step_line("WebFetch https://example.com");
        assert_eq!(other.kind, StepKind::Other);
        assert_eq!(other.done, "WebFetch https://example.com");
    }

    #[test]
    fn an_edit_s_counts_come_off_its_line_and_its_error_stays() {
        let edit = step_line("Asked to edit src/db.rs (+12 \u{2212}3)");
        assert_eq!(
            (edit.done.as_str(), edit.verb.as_str()),
            ("Edited", "Asked to edit")
        );
        assert_eq!(edit.arg, "src/db.rs");
        assert_eq!(edit.counts, Some((12, 3)));

        let denied = step_line("Asked to edit a.rs (+1 \u{2212}0): not allowed");
        assert_eq!(denied.arg, "a.rs: not allowed");
        assert_eq!(denied.counts, Some((1, 0)));

        let codex = step_line("Edited a.rs, b.rs");
        assert_eq!((codex.arg.as_str(), codex.counts), ("a.rs, b.rs", None));
    }

    fn step(summary: &str, pending: bool) -> RunStep {
        RunStep {
            summary: summary.into(),
            ok: true,
            note: false,
            pending,
            user: false,
        }
    }

    #[test]
    fn the_run_says_what_it_is_doing_now() {
        let mut run = RunRecord {
            status: RunStatus::Running,
            ..RunRecord::default()
        };
        assert_eq!(doing_now(&run).0, "Thinking");
        run.steps = vec![step("Read a.rs", false), step("Searched for foo", true)];
        assert_eq!(
            doing_now(&run),
            ("Searching for".into(), "foo".into(), true)
        );
        run.steps[1].pending = false;
        assert_eq!(doing_now(&run).0, "Thinking", "nothing waiting, it thinks");
        run.status = RunStatus::Queued;
        assert_eq!(doing_now(&run).0, "Queued");
    }

    #[test]
    fn calls_between_words_group_together() {
        let note = |text: &str| RunStep {
            summary: text.into(),
            ok: true,
            note: true,
            pending: false,
            user: false,
        };
        let steps = vec![
            note("Looking."),
            step("Read a.rs", false),
            step("Read b.rs", false),
            note("Now the log."),
            step("Ran git log", false),
        ];
        let got = segments(&steps);
        assert_eq!(got.len(), 4);
        assert!(matches!(&got[1], Segment::Tools(calls) if calls.len() == 2));
        assert!(matches!(&got[3], Segment::Tools(calls) if calls.len() == 1));
    }

    #[test]
    fn usage_reads_in_thousands_with_its_cost() {
        let mut run = RunRecord::default();
        assert_eq!(usage_label(&run), None);
        run.usage = Some(perch_github::RunUsage {
            input_tokens: Some(18_000),
            output_tokens: Some(200),
            cached_input_tokens: None,
        });
        assert_eq!(usage_label(&run).as_deref(), Some("18.2k tokens"));
        run.cost_usd = Some(0.214);
        assert_eq!(
            usage_label(&run).as_deref(),
            Some("18.2k tokens \u{00b7} $0.21")
        );
    }

    #[test]
    fn a_finished_run_folds_to_how_long_and_what_it_did() {
        let run = RunRecord {
            status: RunStatus::Completed,
            duration_ms: Some(252_400),
            ..RunRecord::default()
        };
        let steps = vec![step("Read a.rs", false), step("Ran git log", false)];
        assert_eq!(
            fold_words(&run, &steps),
            (
                "Worked for 4m 12s".into(),
                "\u{00b7} read 1 file, ran 1 command".into()
            )
        );
        assert_eq!(
            fold_words(&run, &[]),
            ("Worked for 4m 12s".into(), String::new())
        );
    }

    /// A failed call's line, however long its error, stops at the edge of
    /// the inset, and does not widen the calls beside it.
    #[test]
    fn a_long_failed_step_stays_inside_its_box() {
        use floem::headless::{HeadlessHarness, TestRoot};

        let root = TestRoot::new();
        let failed = |summary: &str| RunStep {
            ok: false,
            ..step(summary, false)
        };
        let calls = vec![
            step("Read src/db.rs", false),
            failed("Ran cat INDEX.md && ls; grep -ril \"with_db_cache\\|share\" . | head: This Bash command contains multiple operations"),
            failed("Bash: This Bash command contains multiple operations. The following part requires approval: cd /Users/someone/.perch/context"),
        ];
        let view = Container::new(tools_box(rows(ReviewState::new(), &calls, false)))
            .debug_name("page")
            .style(|s| s.width(300.0).height_full());
        let mut harness = HeadlessHarness::new_with_size(root, view, 600.0, 300.0);
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
        let right = crate::review::header::tests::rect(&harness, "page").x1;
        let mut ends = Vec::new();
        fn walk(id: floem::ViewId, ends: &mut Vec<f64>) {
            ends.push(id.get_visual_rect_no_clip().x1);
            for child in id.children() {
                walk(child, ends);
            }
        }
        let page = crate::review::header::tests::named(harness.root_id(), "page").unwrap();
        walk(page, &mut ends);
        let widest = ends.into_iter().fold(0.0, f64::max);
        assert!(
            widest <= right + 0.5,
            "a view ends at {widest}, past {right}"
        );
    }
}
