//! A tour stop: one idea with the code that carries it. A read stop shows
//! open, as a card: its claim, Before, Now and Watch, its pictures, the
//! unchanged code it relies on, its asides from the other views, the
//! review threads on its lines, and Done, Come back to this, Chat about
//! this and Next. Consecutive skim stops fold into one group, a row each
//! with the stop's title, its Now sentence and its line count, and a
//! click opens the stop in place.
//!
//! A mark is the reader's: scrolling past a stop records nothing, and
//! Next moves on without marking. A mark keeps the stop's hash, and a
//! stop whose hash changed since shows as changed until marked again.

use std::collections::HashMap;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_changescape::page::{self, hunk, Target, Window};
use perch_changescape::v4::{Aside, Chapter, PicturesItem, Rely, RelyWhy, Stop};
use perch_github::{Anchor, CommentThread, Side, StoredMark, TourMark};

use crate::review::changescape::hunk::hunk_view;
use crate::review::changescape::prose::{self, meta, BODY, MUTED, TITLE};
use crate::review::changescape::side;
use crate::review::changescape::tour::{self, focusable, Ctx};
use crate::review::model::ReviewState;
use crate::review::threads::{self, ThreadFace};
use crate::theme;
use crate::ui::nav_row::thousands;
use crate::ui::widgets::{bordered_button, clickable, notice_box, underlined_link};

/// A read stop, open.
pub fn stop_card(cx: &Ctx, chapter: &Chapter, i: usize) -> AnyView {
    let stop = chapter.stops[i].clone();
    let id = stop.id.clone();
    let view = Stack::vertical((header(cx, &stop), body(cx, chapter, &stop)))
        .debug_name(format!("Changescape stop {id}"))
        .style(|s| {
            s.flex_col()
                .gap(12.0)
                .width_full()
                .padding(theme::CARD_PADDING)
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border_muted())
                .border_radius(theme::CARD_RADIUS)
        })
        .into_any();
    cx.placed.add(Some(id.clone()), view.view_id());
    focusable(cx.state, id, view)
}

/// A run of skim stops: one box with a row for each, a row opening its
/// stop in place.
pub fn skim_group(cx: &Ctx, chapter: &Chapter, group: Vec<usize>) -> impl IntoView {
    let lines: i64 = group.iter().map(|&i| chapter.stops[i].lines).sum();
    let heading = format!(
        "Skim · {} {} · {} lines",
        group.len(),
        if group.len() == 1 { "stop" } else { "stops" },
        thousands(lines)
    );
    let rows: Vec<AnyView> = group
        .into_iter()
        .map(|i| skim_row(cx, chapter, &chapter.stops[i]))
        .collect();
    Stack::vertical((
        meta(heading).style(|s| s.padding_horiz(12.0).padding_vert(8.0)),
        Stack::vertical_from_iter(rows).style(|s| s.flex_col().width_full()),
    ))
    .debug_name("Changescape skim group")
    .style(|s| {
        s.flex_col()
            .width_full()
            .background(theme::canvas_subtle())
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(theme::CARD_RADIUS)
    })
}

fn skim_row(cx: &Ctx, chapter: &Chapter, stop: &Stop) -> AnyView {
    let state = cx.state;
    let placed = cx.placed.clone();
    let id = stop.id.clone();
    let open = {
        let id = id.clone();
        Memo::new(move |_| state.changescape_opened.with(|o| o.contains(&id)))
    };
    let toggle = id.clone();
    let row = Container::new(
        Stack::horizontal((
            Label::derived(move || if open.get() { "▾" } else { "▸" }.to_string()).style(|s| {
                s.width(16.0)
                    .font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .selectable(false)
            }),
            mark_glyph(state, id.clone()),
            Stack::vertical((
                prose::line(&stop.title, BODY),
                stop.now.as_deref().map_or_else(
                    || Empty::new().into_any(),
                    |now| prose::line(now, MUTED).into_any(),
                ),
            ))
            .style(|s| s.flex_col().flex_grow(1.0_f32).min_width(0.0)),
            meta(format!("{} lines", thousands(stop.lines)))
                .style(|s| s.margin_left(12.0).flex_shrink(0.0_f32)),
        ))
        .style(|s| s.items_center().width_full()),
    )
    .debug_name(format!("Changescape skim row {id}"))
    .on_event_stop(el::Click, move |_, _| {
        tour::toggle(state.changescape_opened, &toggle)
    })
    .style(|s| {
        clickable(
            s.width_full()
                .padding_horiz(12.0)
                .padding_vert(8.0)
                .border_top(1.0)
                .border_color(theme::border_muted()),
        )
        .hover(|s| s.background(theme::canvas_hover()))
    });
    let (cx, chapter, stop) = (cx.clone(), chapter.clone(), stop.clone());
    let opened = dyn_container(
        move || open.get(),
        move |open| {
            if open {
                body(&cx, &chapter, &stop)
                    .style(|s| s.padding(12.0).background(theme::canvas()))
                    .into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.width_full());
    let view = Stack::vertical((row, opened))
        .debug_name(format!("Changescape stop {id}"))
        .style(|s| s.flex_col().width_full())
        .into_any();
    placed.add(Some(id.clone()), view.view_id());
    focusable(state, id, view)
}

/// What a stop's mark shows. Tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkShown {
    Mark(Option<TourMark>),
    /// The stop's lines changed since the reader marked it.
    Changed,
}

impl MarkShown {
    pub fn of(state: ReviewState, id: &str) -> MarkShown {
        if state.changescape_changed.with(|c| c.contains_key(id)) {
            return MarkShown::Changed;
        }
        MarkShown::Mark(state.changescape_marks.with(|m| m.get(id).copied()))
    }

    /// A check for done, a return arrow for come back to this, a delta
    /// for changed since its mark, and `none` for no mark.
    pub fn glyph(self, none: &str) -> String {
        match self {
            MarkShown::Mark(Some(TourMark::Done)) => "✓".to_string(),
            MarkShown::Mark(Some(TourMark::Later)) => "↺".to_string(),
            MarkShown::Changed => "Δ".to_string(),
            MarkShown::Mark(None) => none.to_string(),
        }
    }
}

/// The stop's mark: see `MarkShown::glyph`, with a ring for no mark.
fn mark_glyph(state: ReviewState, id: String) -> impl IntoView {
    let mark = Memo::new(move |_| MarkShown::of(state, &id));
    Label::derived(move || mark.get().glyph("○"))
        .debug_name("Changescape mark")
        .style(move |s| {
            s.width(20.0)
                .font_size(theme::TEXT_BODY)
                .color(match mark.get() {
                    MarkShown::Mark(Some(TourMark::Done)) => theme::open(),
                    MarkShown::Mark(Some(TourMark::Later)) | MarkShown::Changed => {
                        theme::notice_fg()
                    }
                    MarkShown::Mark(None) => theme::fg_subtle(),
                })
                .selectable(false)
                .flex_shrink(0.0_f32)
        })
}

/// A read stop's first row: its mark, its claim, and its line count.
fn header(cx: &Ctx, stop: &Stop) -> impl IntoView {
    Stack::horizontal((
        mark_glyph(cx.state, stop.id.clone()).style(|s| s.margin_top(2.0)),
        Container::new(prose::paragraph(&stop.title, TITLE))
            .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0)),
        meta(format!("{} lines", thousands(stop.lines)))
            .style(|s| s.margin_left(12.0).margin_top(3.0).flex_shrink(0.0_f32)),
    ))
    .style(|s| s.items_start().width_full())
}

/// Everything under a stop's title: its text, its pictures, the threads
/// on its lines, the composer it opened, and its buttons.
fn body(cx: &Ctx, chapter: &Chapter, stop: &Stop) -> Stack {
    let mut parts: Vec<AnyView> = vec![changed_notice(cx.state, stop.id.clone()).into_any()];
    for (label, text) in [
        ("Before", stop.before.as_deref()),
        ("Now", stop.now.as_deref()),
        ("Watch", stop.watch.as_deref()),
    ] {
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            parts.push(field(label, text).into_any());
        }
    }
    if stop.watch.is_some() {
        parts.push(
            Container::new(tour::reference(
                cx,
                stop.watch_target.as_ref().map(Target::from),
            ))
            .style(|s| s.padding_left(FIELD_LABEL))
            .into_any(),
        );
    }
    for (i, picture) in stop.pictures.iter().enumerate() {
        match picture {
            PicturesItem::Hunk { .. } => {
                if let Some((file, windows)) = hunk(picture) {
                    parts.push(
                        hunk_view(cx.state, cx.backend.clone(), file.to_string(), windows)
                            .into_any(),
                    );
                }
            }
            PicturesItem::Seq {
                caption,
                title,
                note,
                ..
            }
            | PicturesItem::Types {
                caption,
                title,
                note,
                ..
            } => {
                if let Some(figure) = page::picture_figure(picture) {
                    parts.push(
                        tour::diagram(
                            cx,
                            tour::picture_key(&stop.id, i),
                            caption.as_deref().or(title.as_deref()),
                            note.as_deref(),
                            figure,
                        )
                        .into_any(),
                    );
                }
            }
        }
    }
    if !stop.relies.is_empty() {
        parts.push(relies(cx, &stop.relies).into_any());
    }
    if !stop.asides.is_empty() {
        parts.push(asides(cx, &stop.asides).into_any());
    }
    parts.push(stop_threads(cx, stop).into_any());
    parts.push(composer(cx, stop).into_any());
    parts.push(buttons(cx, chapter, stop).into_any());
    Stack::vertical_from_iter(parts).style(|s| s.flex_col().gap(10.0).width_full())
}

/// The width of a field's label.
const FIELD_LABEL: f64 = 64.0;

fn field(label: &'static str, text: &str) -> impl IntoView {
    Stack::horizontal((
        Label::new(label).style(|s| {
            s.width(FIELD_LABEL)
                .flex_shrink(0.0_f32)
                .font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_muted())
                .margin_top(2.0)
                .selectable(false)
        }),
        Container::new(prose::paragraph(text, BODY))
            .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0)),
    ))
    .debug_name(format!("Changescape field {}", label.to_lowercase()))
    .style(|s| s.items_start().width_full())
}

/// What a rely's entry says the stop's lines rely on: the narration's
/// sentence, else the first sentence of the symbol's doc comment.
pub fn rely_says(rely: &Rely) -> Option<String> {
    if let Some(what) = rely.what.as_deref().filter(|w| !w.is_empty()) {
        return Some(what.to_string());
    }
    let doc = rely.doc.as_deref()?.trim();
    let end = doc
        .char_indices()
        .find(|&(i, c)| {
            matches!(c, '.' | '!' | '?')
                && doc[i + 1..].chars().next().is_none_or(char::is_whitespace)
        })
        .map_or(doc.len(), |(i, _)| i + 1);
    Some(doc[..end].to_string()).filter(|s| !s.is_empty())
}

/// How a stop relies on the code: `calls`, `implements` or `matches`.
/// A reliance with no kind reads as calls.
pub fn rely_why(rely: &Rely) -> &'static str {
    match rely.why {
        Some(RelyWhy::Matches) => "matches",
        Some(RelyWhy::Implements) => "implements",
        Some(RelyWhy::Calls) | None => "calls",
    }
}

/// The unchanged code a stop's new lines rely on, after its pictures: each
/// entry's symbol, which opens it, and what it promises. Code opens the
/// symbol's lines from the checkout, in place.
fn relies(cx: &Ctx, relies: &[Rely]) -> impl IntoView {
    let mut parts: Vec<AnyView> = vec![Label::new("Relies on")
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_muted())
                .selectable(false)
        })
        .into_any()];
    for rely in relies {
        parts.push(rely_view(cx, rely));
    }
    Stack::vertical_from_iter(parts)
        .debug_name("Changescape relies")
        .style(|s| s.flex_col().gap(6.0).width_full())
}

fn rely_view(cx: &Ctx, rely: &Rely) -> AnyView {
    let title = cx
        .page
        .elements
        .get(&rely.element)
        .map_or_else(|| rely.element.clone(), |e| e.title.clone());
    let open = RwSignal::new(false);
    let name = {
        let (cx, el) = (cx.clone(), rely.element.clone());
        let held = cx.page.elements.contains_key(&el);
        Container::new(Label::new(title).style(move |s| {
            s.font_size(theme::TEXT_META)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(if held { theme::link() } else { theme::fg() })
                .selectable(false)
        }))
        .debug_name("Changescape rely element")
        .on_event_stop(el::Click, move |_, _| cx.open(&el))
        .style(move |s| s.apply_if(held, |s| clickable(underlined_link(s))))
    };
    let why = Label::new(rely_why(rely)).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::fg_subtle())
            .margin_left(8.0)
            .selectable(false)
    });
    let code = Container::new(
        Label::derived(move || if open.get() { "▾ Code" } else { "▸ Code" }.to_string()).style(
            |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::fg_muted())
                    .selectable(false)
            },
        ),
    )
    .debug_name("Changescape rely code toggle")
    .on_event_stop(el::Click, move |_, _| open.update(|o| *o = !*o))
    .style(|s| clickable(s.margin_left(12.0)));
    let says = rely_says(rely).map_or_else(
        || Empty::new().into_any(),
        |says| prose::paragraph(&says, MUTED).into_any(),
    );
    let (state, backend) = (cx.state, cx.backend.clone());
    let file = rely.code.file.clone();
    let window = Window {
        base: None,
        head: page::lines(&rely.code.head),
    };
    let body = dyn_container(
        move || open.get(),
        move |open| {
            if open && window.head.is_some() {
                hunk_view(state, backend.clone(), file.clone(), vec![window]).into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.width_full());
    Stack::vertical((
        Stack::horizontal((name, why, code)).style(|s| s.items_center().width_full()),
        says,
        body,
    ))
    .debug_name(format!("Changescape rely {}", rely.element))
    .style(|s| {
        s.flex_col()
            .gap(2.0)
            .width_full()
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(theme::border_muted())
    })
    .into_any()
}

/// Context from the other views that matters at the stop, after what it
/// relies on: one quiet row each, the question, its answer, and the
/// elements of the view's rows.
fn asides(cx: &Ctx, asides: &[Aside]) -> impl IntoView {
    let rows: Vec<AnyView> = asides
        .iter()
        .map(|aside| {
            Stack::vertical((
                prose::paragraph(
                    &aside.question,
                    prose::Ink {
                        weight: theme::weight_emphasis,
                        ..MUTED
                    },
                ),
                prose::paragraph(&aside.answer, MUTED),
                tour::element_rows(cx, &aside.elements),
            ))
            .debug_name("Changescape aside")
            .style(|s| s.flex_col().gap(2.0).width_full().padding_vert(3.0))
            .into_any()
        })
        .collect();
    Stack::vertical_from_iter(rows)
        .debug_name("Changescape asides")
        .style(|s| {
            s.flex_col()
                .width_full()
                .padding_top(6.0)
                .border_top(1.0)
                .border_color(theme::border_muted())
        })
}

/// Each hunk picture's file and windows.
fn windows(stop: &Stop) -> Vec<(&str, Vec<Window>)> {
    stop.pictures.iter().filter_map(hunk).collect()
}

/// The threads whose lines a stop's windows hold: a thread on the head on
/// a head window's lines, one on the base on a base window's, in the
/// order the threads come.
pub fn threads_on<'a>(stop: &Stop, threads: &'a [CommentThread]) -> Vec<&'a CommentThread> {
    let shown = windows(stop);
    threads
        .iter()
        .filter(|thread| {
            let Some(anchor) = &thread.anchor else {
                return false;
            };
            shown.iter().any(|(file, windows)| {
                *file == anchor.path
                    && windows.iter().any(|w| {
                        let range = match anchor.side {
                            Side::Right => w.head,
                            Side::Left => w.base,
                        };
                        range.is_some_and(|(first, last)| {
                            anchor.start_line.min(anchor.line) <= last
                                && first <= anchor.start_line.max(anchor.line)
                        })
                    })
            })
        })
        .collect()
}

/// The review threads on the stop's lines, GitHub's and Perch's, as
/// Conversation draws them.
fn stop_threads(cx: &Ctx, stop: &Stop) -> impl IntoView {
    let (state, stop) = (cx.state, stop.clone());
    let on = Memo::new(move |_| {
        state.comment_threads.with(|all| {
            threads_on(&stop, all)
                .into_iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>()
        })
    });
    let (app, backend) = (cx.app, cx.backend.clone());
    dyn_container(
        move || on.get(),
        move |ids| {
            let all = state.comment_threads.get_untracked();
            let open_file = crate::review::open_file(state, backend.clone());
            let cards: Vec<AnyView> = ids
                .iter()
                .filter_map(|id| all.iter().find(|t| &t.id == id).cloned())
                .map(|thread| {
                    crate::review::cards::thread_card(
                        app,
                        state,
                        backend.clone(),
                        open_file.clone(),
                        thread,
                    )
                    .into_any()
                })
                .collect();
            Stack::vertical_from_iter(cards)
                .style(|s| s.flex_col().gap(theme::CARD_GAP).width_full())
                .into_any()
        },
    )
    .debug_name("Changescape stop threads")
    .style(|s| s.width_full())
}

/// Where a stop's composer goes: what `Composer.about` says for it.
pub fn about(stop: &str) -> String {
    format!("tour:{stop}")
}

/// The composer Chat about this opened on this stop, in place.
fn composer(cx: &Ctx, stop: &Stop) -> impl IntoView {
    let (app, state, backend) = (cx.app, cx.state, cx.backend.clone());
    let key = about(&stop.id);
    let open = Memo::new(move |_| {
        state.composer.with(|c| {
            c.as_ref()
                .filter(|c| c.about.as_deref() == Some(key.as_str()))
                .map(|c| c.opening)
        })
    });
    dyn_container(
        move || open.get(),
        move |open| match (open, state.composer.get_untracked()) {
            (Some(_), Some(c)) => {
                threads::composer_view(app, state, backend.clone(), c, ThreadFace::Card).into_any()
            }
            _ => Empty::new().into_any(),
        },
    )
    .debug_name("Changescape stop composer")
    .style(|s| s.width_full())
}

/// What a question about a stop carries: the chapter and the stop's
/// text, and where its code is.
pub fn chat_context(chapter: &Chapter, stop: &Stop) -> String {
    let mut out = format!(
        "The reader is on a stop of the Changescape tour of this pull request.\nChapter: {}\nStop: {}\n",
        prose::plain(&chapter.title),
        prose::plain(&stop.title)
    );
    for (label, text) in [
        ("Before", &stop.before),
        ("Now", &stop.now),
        ("Watch", &stop.watch),
    ] {
        if let Some(text) = text {
            out.push_str(&format!("{label}: {}\n", prose::plain(text)));
        }
    }
    let shown = windows(stop);
    if !shown.is_empty() {
        out.push_str("The stop's code, as line ranges at the merge base and at the head:\n");
        for (file, windows) in shown {
            for w in windows {
                let side = |name: &str, range: Option<(u32, u32)>| {
                    range.map(|(a, b)| format!("{name} {a}-{b}"))
                };
                let sides: Vec<String> = [side("base", w.base), side("head", w.head)]
                    .into_iter()
                    .flatten()
                    .collect();
                out.push_str(&format!("- {file}: {}\n", sides.join(", ")));
            }
        }
    }
    out
}

/// The lines a question about a stop is anchored to: its first head
/// window.
pub fn chat_anchor(stop: &Stop, head_sha: &str) -> Option<Anchor> {
    windows(stop).into_iter().find_map(|(file, windows)| {
        let (first, last) = windows.iter().find_map(|w| w.head)?;
        Some(Anchor {
            path: file.to_string(),
            start_line: first,
            line: last,
            side: Side::Right,
            commit_sha: head_sha.to_string(),
        })
    })
}

/// The stored marks as the tour shows them: the marks that hold, and the
/// marks on stops whose lines changed since. A mark is changed when the
/// stop's hash on the page is not the one the mark kept. A mark or a stop
/// with no hash holds.
pub fn split_marks(
    page: Option<&std::sync::Arc<perch_changescape::LoadedPage>>,
    stored: &HashMap<String, StoredMark>,
) -> (HashMap<String, TourMark>, HashMap<String, TourMark>) {
    let mut marks = HashMap::new();
    let mut changed = HashMap::new();
    for (id, StoredMark { mark, h }) in stored {
        let now = page
            .and_then(|p| p.stop(id))
            .and_then(|(_, s)| s.hash.as_ref());
        match (h, now) {
            (Some(then), Some(now)) if then != now => changed.insert(id.clone(), *mark),
            _ => marks.insert(id.clone(), *mark),
        };
    }
    (marks, changed)
}

/// Mark a stop, or clear its mark, here and in the database, keeping the
/// stop's hash with the mark. Either way the stop is no longer changed
/// since its mark.
fn set_mark(cx: &Ctx, stop: &Stop, mark: Option<TourMark>) {
    let id = stop.id.clone();
    cx.state.changescape_marks.update(|marks| match mark {
        Some(mark) => {
            marks.insert(id.clone(), mark);
        }
        None => {
            marks.remove(&id);
        }
    });
    if cx
        .state
        .changescape_changed
        .with_untracked(|c| c.contains_key(&id))
    {
        cx.state.changescape_changed.update(|c| {
            c.remove(&id);
        });
    }
    let number = u64::try_from(cx.page.meta.number).unwrap_or_default();
    let stored = mark.map(|mark| StoredMark {
        mark,
        h: stop.hash.clone(),
    });
    cx.backend
        .set_tour_mark(&cx.page.meta.repo, number, &id, stored);
}

/// The line over a stop whose lines changed since the reader marked it.
fn changed_notice(state: ReviewState, id: String) -> impl IntoView {
    let then = Memo::new(move |_| state.changescape_changed.with(|c| c.get(&id).copied()));
    Label::derived(move || match then.get() {
        Some(TourMark::Done) => "Changed since you marked it done.".to_string(),
        Some(TourMark::Later) => "Changed since you marked it to come back to.".to_string(),
        None => String::new(),
    })
    .debug_name("Changescape changed since marked")
    .style(move |s| {
        notice_box(
            s.font_size(theme::TEXT_META)
                .color(theme::notice_fg())
                .width_full()
                .padding_vert(4.0)
                .padding_horiz(8.0),
        )
        .apply_if(then.get().is_none(), |s| s.hide())
    })
}

/// Done and Come back to this, each a toggle, then Chat about this, and
/// Next, which moves on and marks nothing.
fn buttons(cx: &Ctx, chapter: &Chapter, stop: &Stop) -> impl IntoView {
    let state = cx.state;
    let id = stop.id.clone();
    let mark = {
        let id = id.clone();
        Memo::new(move |_| state.changescape_marks.with(|m| m.get(&id).copied()))
    };
    let button = |label: &'static str, name: &'static str| {
        Container::new(Label::new(label).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg())
                .selectable(false)
        }))
        .debug_name(name)
        .style(bordered_button)
    };
    let toggle = |label: &'static str, name: &'static str, this: TourMark| {
        let (cx, stop) = (cx.clone(), stop.clone());
        button(label, name)
            .on_event_stop(el::Click, move |_, _| {
                let next = (mark.get_untracked() != Some(this)).then_some(this);
                set_mark(&cx, &stop, next);
            })
            .style(move |s| {
                s.apply_if(mark.get() == Some(this), |s| {
                    s.background(theme::accent_subtle())
                        .border_color(theme::accent_ring())
                })
            })
    };
    let chat = {
        let (app, page, chapter, stop) = (cx.app, cx.page.clone(), chapter.clone(), stop.clone());
        button("Chat about this", "Changescape chat").on_event_stop(el::Click, move |_, _| {
            let head = state
                .pull_request
                .with_untracked(|p| p.as_ref().map(|p| p.head_sha.clone()))
                .unwrap_or_else(|| page.meta.head.sha.clone());
            state.open_composer_about(
                about(&stop.id),
                chat_anchor(&stop, &head),
                chat_context(&chapter, &stop),
                threads::defaults(app),
            );
        })
    };
    let (label, to) = match page::next_read(cx.tour(), &id) {
        Some(next) => ("Next", next),
        None => ("Where you stand", side::ENDING.to_string()),
    };
    let next = button(label, "Changescape next")
        .on_event_stop(el::Click, move |_, _| tour::go_to(state, &to));
    Stack::horizontal((
        toggle("Done", "Changescape done", TourMark::Done),
        toggle("Come back to this", "Changescape later", TourMark::Later),
        chat,
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        next,
    ))
    .style(|s| s.gap(8.0).items_center().width_full().margin_top(4.0))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn stop() -> Stop {
        serde_json::from_value(serde_json::json!({
            "id": "s", "title": "The `loop` asks first", "elements": [], "lines": 4,
            "asides": [], "before": null, "files": [], "hash": null, "relies": [],
            "verdict": "read", "watch": null, "watch_target": null,
            "now": "It asks the `policy`.",
            "pictures": [
                { "kind": "seq", "caption": null, "hot": {}, "note": null,
                  "svg": "<svg/>", "title": null },
                { "kind": "hunk", "caption": null, "file": "a.rs", "note": null,
                  "title": null, "windows": [
                    { "base": { "start": 10, "end": 12 }, "head": { "start": 20, "end": 30 } },
                    { "base": null, "head": { "start": 50, "end": 52 } } ] },
            ],
        }))
        .unwrap()
    }

    /// A thread from GitHub with no comments, on `lines` of `path` on
    /// `side`.
    pub(in crate::review::changescape) fn thread(
        id: &str,
        path: &str,
        lines: (u32, u32),
        side: Side,
    ) -> CommentThread {
        CommentThread {
            id: id.into(),
            worktree_id: String::new(),
            origin: perch_github::ThreadOrigin::GitHub,
            github_id: None,
            anchor: Some(Anchor {
                path: path.into(),
                start_line: lines.0,
                line: lines.1,
                side,
                commit_sha: String::new(),
            }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: jiff::Timestamp::UNIX_EPOCH,
            messages: Vec::new(),
        }
    }

    #[test]
    fn a_thread_lands_on_the_stop_whose_window_holds_its_lines_on_its_side() {
        let threads = vec![
            thread("head", "a.rs", (29, 33), Side::Right),
            thread("base", "a.rs", (11, 11), Side::Left),
            thread("between", "a.rs", (40, 41), Side::Right),
            thread("base-on-head", "a.rs", (51, 51), Side::Left),
            thread("other", "b.rs", (21, 21), Side::Right),
        ];
        let on: Vec<&str> = threads_on(&stop(), &threads)
            .into_iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(on, ["head", "base"]);
    }

    #[test]
    fn a_rely_says_the_narrations_sentence_else_its_docs_first() {
        let rely = |what: Option<&str>, doc: &str, why: Option<&str>| -> Rely {
            serde_json::from_value(serde_json::json!({
                "element": "fn:x", "what": what, "doc": doc, "why": why, "signature": null,
                "code": { "file": "a.rs", "head": { "start": 1, "end": 2 } } }))
            .unwrap()
        };
        assert_eq!(
            rely_says(&rely(Some("It never blocks."), "Doc.", None)).as_deref(),
            Some("It never blocks.")
        );
        assert_eq!(
            rely_says(&rely(None, "Reads v1.2 of the file. Then more.", None)).as_deref(),
            Some("Reads v1.2 of the file.")
        );
        assert_eq!(rely_says(&rely(None, "", None)), None);
        assert_eq!(rely_why(&rely(None, "", Some("implements"))), "implements");
        assert_eq!(rely_why(&rely(None, "", Some("matches"))), "matches");
        assert_eq!(rely_why(&rely(None, "", None)), "calls");
    }

    #[test]
    fn a_question_carries_the_stops_text_and_windows_and_anchors_to_its_first_head_window() {
        let chapter: Chapter = serde_json::from_value(serde_json::json!({
            "id": "c", "title": "The claim", "verdict": "read", "stops": [], "hash": null,
            "intro": null, "lines": { "read": 0, "skim": 0 }, "parts": [], "sections": [],
            "spine": null }))
        .unwrap();
        let context = chat_context(&chapter, &stop());
        assert!(context.contains("Stop: The loop asks first"));
        assert!(context.contains("Now: It asks the policy."));
        assert!(context.contains("- a.rs: base 10-12, head 20-30"));
        assert!(context.contains("- a.rs: head 50-52"));
        let anchor = chat_anchor(&stop(), "h").unwrap();
        assert_eq!(
            (anchor.path.as_str(), anchor.start_line, anchor.line),
            ("a.rs", 20, 30)
        );
    }
}
