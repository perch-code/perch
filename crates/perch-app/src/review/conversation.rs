//! Conversation: the item's description, everything said about it on
//! GitHub, and every thread the reader has had with an agent about it, in
//! one quiet reading column. Under the summary comes what Perch is doing
//! about the item, its agents and the forms the menus unfold; then the
//! description, the composer, and the timeline, newest first, its older
//! cards folded under a row that shows them.
//!
//! The order is `review::timeline`'s, oldest first, which the page turns
//! round as it draws it. The cards are in `review::cards`, and a thread's
//! card shares its views with the inline card on Code.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, ReactiveId, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::scroll::ScrollChanged;
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};
use floem::ViewId;

use crate::backend::Backend;
use crate::review::cards;
use crate::review::doc::CardId;
use crate::review::model::{Paging, ReviewFindingFilter, ReviewState};
use crate::review::threads::{self, ThreadFace};
use crate::review::timeline::{timeline, Card};
use crate::review::OpenFile;
use crate::review::{conversation_find, find};
use crate::state::AppState;
use crate::theme;

/// How many of the newest cards the timeline shows before the rest fold
/// under Show older: enough to catch up on what happened lately, few
/// enough that a long history does not push the page's end out of reach.
const NEWEST_SHOWN: usize = 8;

/// What the reader unfolded on the page: the timeline's older cards, and
/// the whole of a long description.
#[derive(Clone, Copy)]
struct Unfolded {
    older: RwSignal<bool>,
    description: RwSignal<bool>,
}

thread_local! {
    /// Each window's unfolding, by its review state, with the item it is
    /// for. The page is built again whenever Conversation comes back from
    /// Code, and the scroll it is put back to was taken with these open,
    /// so they outlast the page; another item folds them again.
    static UNFOLDED: RefCell<HashMap<ReactiveId, (String, bool, bool)>> =
        RefCell::new(HashMap::new());
}

/// The page's unfolding, as the last page for this item left it.
fn unfolded(state: ReviewState) -> Unfolded {
    let window = state.item.id();
    let unfolded = Unfolded {
        older: RwSignal::new(false),
        description: RwSignal::new(false),
    };
    let item = Memo::new(move |_| {
        state
            .item
            .with(|item| item.as_ref().map(|item| item.id.clone()))
    });
    Effect::new(move |_| {
        let item = item.get();
        let (older, description) =
            UNFOLDED.with(|kept| match (kept.borrow().get(&window), &item) {
                (Some((was, older, description)), Some(item)) if was == item => {
                    (*older, *description)
                }
                _ => (false, false),
            });
        if unfolded.older.get_untracked() != older {
            unfolded.older.set(older);
        }
        if unfolded.description.get_untracked() != description {
            unfolded.description.set(description);
        }
    });
    Effect::new(move |_| {
        let (older, description) = (unfolded.older.get(), unfolded.description.get());
        let item = item.get_untracked();
        UNFOLDED.with(|kept| {
            let mut kept = kept.borrow_mut();
            match item {
                Some(item) if older || description => {
                    kept.insert(window, (item, older, description));
                }
                _ => {
                    kept.remove(&window);
                }
            }
        });
    });
    unfolded
}

/// The timeline as the page shows it: newest first, and without the
/// description, which is pinned above it.
fn newest_first(cards: &[Card]) -> impl Iterator<Item = &Card> {
    cards
        .iter()
        .rev()
        .filter(|card| !matches!(card, Card::Description { .. }))
}

/// Whether the card `id` is one that Show older would show.
fn folded_away(cards: &[Card], id: &CardId) -> bool {
    newest_first(cards)
        .skip(NEWEST_SHOWN)
        .any(|card| cards::card_id(card) == *id)
}

/// The card a composer is open in: a thread with its follow-up box open
/// under it, or a comment or review being edited in place.
#[derive(Clone, PartialEq)]
enum Held {
    Thread(String),
    Edit(String),
}

impl Held {
    /// What the open composer is in, when it is in a card.
    fn of(state: ReviewState) -> Option<Held> {
        state.composer.with(|c| {
            let c = c.as_ref()?;
            match (&c.thread_id, &c.editing) {
                (Some(thread), _) => Some(Held::Thread(thread.clone())),
                (None, Some(node)) => Some(Held::Edit(node.clone())),
                (None, None) => None,
            }
        })
    }

    fn is(&self, card: &Card) -> bool {
        match (self, card) {
            (Held::Thread(id), Card::Thread(thread)) => thread.id == *id,
            (Held::Edit(node), Card::Comment(comment)) => comment.node_id == *node,
            (Held::Edit(node), Card::Review { review }) => review.node_id == *node,
            _ => false,
        }
    }
}

/// Whether the timeline shows every card: the reader unfolded them, or
/// newer cards have pushed the one a composer is open in past the
/// newest. That card is not folded away under the reader as they type
/// in it, which would take the box and the keyboard with it.
fn shows_all(cards: &[Card], older: bool, held: Option<&Held>) -> bool {
    older
        || held.is_some_and(|held| {
            newest_first(cards)
                .skip(NEWEST_SHOWN)
                .any(|card| held.is(card))
        })
}

/// The room above each timeline card, by its key: cards sit the standard
/// gap apart, and an event line, which has no box, a little closer to
/// whatever is on either side of it. Apart from the cards' keys, and
/// read as each card is styled, so a newer card landing over the one
/// that was first moves it down without building it again, along with
/// whatever is open in it.
fn gaps(cards: &[Card]) -> HashMap<String, f64> {
    let mut gaps = HashMap::new();
    // Whether the card over this one was an event line; `None` for the
    // first.
    let mut after_event = None;
    for card in newest_first(cards) {
        let event = matches!(card, Card::Event(_));
        let gap = match after_event {
            None => 0.0,
            Some(was) if was || event => theme::EVENT_GAP,
            Some(_) => theme::CARD_GAP,
        };
        after_event = Some(event);
        gaps.insert(cards::key(card), gap);
    }
    gaps
}

/// Where the reader is on the page, so what they are looking at stays
/// where it is in the window when something over it changes height. The
/// timeline is newest first, so a comment a poll brings, a thread an Ask
/// starts and an answer streaming into it all land over a reader who has
/// scrolled down to older cards; and a form over everything as it
/// unfolds. floem's scroll keeps its offset, not what is at it, so without
/// this each would push the page down under the reader.
#[derive(Default)]
struct Place {
    /// The scroll's offset, as it last said.
    offset: Cell<f64>,
    /// The column's rows in sight, top first, each with its top on the
    /// page as it was last laid out.
    seen: RefCell<Vec<(ViewId, f64)>>,
}

impl Place {
    /// The column's rows, each of the timeline's cards a row of its own,
    /// with their tops and bottoms on the page. Hidden ones have no
    /// height.
    fn rows(column: ViewId, timeline: ViewId) -> Vec<(ViewId, f64, f64)> {
        let top = column.get_layout_rect().y0;
        let mut rows = Vec::new();
        for row in column.children() {
            let rect = row.get_layout_rect();
            if row == timeline {
                for card in row.children() {
                    let card_rect = card.get_layout_rect();
                    let y = top + rect.y0;
                    rows.push((card, y + card_rect.y0, y + card_rect.y1));
                }
            } else {
                rows.push((row, top + rect.y0, top + rect.y1));
            }
        }
        rows
    }

    /// Takes note of the rows in sight with the page scrolled to `offset`.
    fn see(&self, rows: &[(ViewId, f64, f64)], offset: f64, height: f64) {
        self.offset.set(offset);
        *self.seen.borrow_mut() = rows
            .iter()
            .filter(|(_, y0, y1)| y1 > y0 && *y1 > offset && *y0 < offset + height)
            .map(|(id, y0, _)| (*id, *y0))
            .collect();
    }

    /// How far the topmost row the reader had in sight that is still on
    /// the page has moved since. Nothing at the page's top, where what
    /// lands over the reader is what they should see.
    fn moved(&self, rows: &[(ViewId, f64, f64)]) -> Option<f64> {
        if self.offset.get() <= 0.0 {
            return None;
        }
        self.seen
            .borrow()
            .iter()
            .find_map(|(id, was)| {
                rows.iter()
                    .find(|(row, y0, y1)| row == id && y1 > y0)
                    .map(|(_, now, _)| now - was)
            })
            .filter(|moved| moved.abs() >= 0.5)
    }
}

/// Shows the older cards whenever something asks for one of them: a
/// question's run from the Agents dropdown or a notification's open, which name
/// the card to bring into view, and Cmd-F, which looks through every
/// card on the page. And keeps them shown once newer cards have pushed
/// the one a composer is open in among them, so it does not fold away
/// when the composer closes either.
fn unfold_when_asked(
    state: ReviewState,
    all: Memo<Vec<Card>>,
    held: Memo<Option<Held>>,
    unfolded: Unfolded,
) {
    let unfold = move || {
        if !unfolded.older.get_untracked() {
            unfolded.older.set(true);
        }
    };
    // A card asked for once it is laid out, which a folded card never is.
    // Watched as the cards change too, since one asked for before it
    // landed may land among the older.
    Effect::new(move |_| {
        let asked = state.pending_focus_card.get();
        let folded = asked.is_some_and(|id| {
            !unfolded.older.get_untracked() && all.with(|cards| folded_away(cards, &id))
        });
        if folded {
            unfold();
        }
    });
    // A card asked for at once, which scrolls itself into view only if it
    // was there to hear it: the folded one is asked again once it is
    // built. The first run only subscribes, as the cards' own do, so a
    // card focused on Code does not move the page as it comes up.
    Effect::new(move |first: Option<()>| {
        let asked = state.focus_card.get();
        if first.is_none() || unfolded.older.get_untracked() {
            return;
        }
        let Some(id) = asked else {
            return;
        };
        if all.with_untracked(|cards| folded_away(cards, &id)) {
            unfold();
            state.pending_focus_card.set(Some(id));
        }
    });
    Effect::new(move |_| {
        if state.conversation_find_open.get() {
            unfold();
        }
    });
    Effect::new(move |_| {
        let held = held.get();
        if all.with(|cards| shows_all(cards, false, held.as_ref())) {
            unfold();
        }
    });
}

/// What Perch is doing about the item, under the summary: the forms the
/// menus unfold. Its merge is the bar's Merge button's, its agents' runs
/// the bar's Agents dropdown's, and their reviews the Review popover's.
/// Each puts room above
/// itself; the group gives that room back at its top and keeps it at its
/// foot, so with none of them showing the description is where it would
/// be without them.
fn sections(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::vertical((
        crate::review::open_pull::surface(app, state, backend.clone()),
        crate::review::close_as_issue::surface(app, state, backend),
    ))
    .debug_name("conversation sections")
    .style(|s| {
        s.flex_col()
            .width_full()
            .margin_top(-theme::CONVERSATION_ENTRY_PADDING)
            .padding_bottom(theme::CONVERSATION_ENTRY_PADDING)
    })
}

/// Under the newest cards, the way to the rest: a quiet button between
/// two rules, there while any are folded away.
fn older_row(hidden: Memo<usize>, older: RwSignal<bool>) -> impl IntoView {
    let rule = || {
        Empty::new().style(|s| {
            s.height(1.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .background(theme::border_muted())
        })
    };
    Stack::horizontal((
        rule(),
        cards::text_action(
            move || format!("Show {} older", hidden.get()),
            "show-older",
            move || older.set(true),
        ),
        rule(),
    ))
    .debug_name("older cards")
    .style(move |s| {
        s.items_center()
            .width_full()
            .gap(8.0)
            .margin_top(theme::CARD_GAP)
            .apply_if(hidden.get() == 0, |s| s.hide())
    })
}

/// One scrolling column, centred, on the page grey, with the find bar
/// hung over its top right.
pub fn view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    let description_backend = backend.clone();
    let description_file = open_file.clone();
    let card_backend = backend.clone();
    let composer_backend = backend.clone();
    // Every card, oldest first, as the timeline sorts them; a memo, so a
    // write that changes no card leaves the page alone.
    let all = Memo::new(move |_| cards(state));
    let unfolded = unfolded(state);
    let held = Memo::new(move |_| Held::of(state));
    unfold_when_asked(state, all, held, unfolded);
    let gaps = Memo::new(move |_| all.with(|cards| gaps(cards)));
    let description = move || {
        all.with(|cards| {
            cards
                .iter()
                .filter(|card| matches!(card, Card::Description { .. }))
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    // Whether a card holding a composer has been pushed among the older
    // is worked out here, from the cards and the composer as they are
    // now, rather than left to the effect that unfolds them: run first,
    // this would drop that card and build it again.
    let rows = move || {
        let older = unfolded.older.get();
        let held = held.get();
        all.with(|cards| {
            let shown = if shows_all(cards, older, held.as_ref()) {
                usize::MAX
            } else {
                NEWEST_SHOWN
            };
            newest_first(cards).take(shown).cloned().collect::<Vec<_>>()
        })
    };
    let hidden = Memo::new(move |_| {
        let older = unfolded.older.get();
        let held = held.get();
        all.with(|cards| {
            if shows_all(cards, older, held.as_ref()) {
                0
            } else {
                newest_first(cards).count().saturating_sub(NEWEST_SHOWN)
            }
        })
    });
    let empty = move || {
        // Only a bare branch has nothing: a pull request or an issue has a
        // description whatever else it has.
        state.comment_threads.with(|t| t.is_empty())
            && state.commits.with(|c| c.is_empty())
            && state.pull_request.with(Option::is_none)
            && state.issue.with(Option::is_none)
            && state
                .item
                .with(|item| item.as_ref().map(|item| item.id.clone()))
                .is_none_or(|item_id| app.review_groups_for(&item_id).is_empty())
    };

    let timeline = dyn_stack(rows, cards::key, move |card| {
        let key = cards::key(&card);
        let gap = move || gaps.with(|gaps| gaps.get(&key).copied().unwrap_or(0.0));
        card_view(
            app,
            state,
            card_backend.clone(),
            open_file.clone(),
            card,
            gap,
            unfolded,
        )
    })
    .debug_name("timeline")
    .style(|s| s.flex_col().width_full());
    let timeline_id = timeline.id();

    let column = Stack::vertical((
        crate::review::summary::summary(app, state, backend.clone()),
        sections(app, state, backend.clone()),
        // The description first whatever else there is: it says what
        // the item is, and the timeline under it is what came of it.
        dyn_stack(description, cards::key, move |card| {
            card_view(
                app,
                state,
                description_backend.clone(),
                description_file.clone(),
                card,
                || 0.0,
                unfolded,
            )
        })
        .style(|s| s.flex_col().width_full()),
        composer(app, state, composer_backend).style(|s| {
            s.width_full()
                .padding_top(theme::CARD_GAP)
                .padding_bottom(theme::CARD_GAP)
        }),
        timeline,
        older_row(hidden, unfolded.older),
        resolved_row(app, state, unfolded.older),
        Label::derived(|| "Nothing here yet. Ask about the worktree above.".to_string()).style(
            move |s| {
                // Hidden rather than absent, so it takes no share of the
                // column's gap while there is something here.
                s.font_size(theme::TEXT_BODY)
                    .color(theme::fg_subtle())
                    .width_full()
                    .text_align(floem::text::Alignment::Center)
                    .padding_vert(28.0)
                    .selectable(false)
                    .apply_if(!empty(), |s| s.hide())
            },
        ),
    ))
    .debug_name("conversation column")
    // A definite width, cut to the page's when the page is narrower,
    // rather than the page's width capped: under a cap, the pass that
    // sizes the page unconstrained lands on the cap's width as well,
    // and taffy takes its unwrapped height for the constrained pass
    // at that same width. The column's bottom, wrapped rows and long
    // comments included, could then not be scrolled into view.
    .style(|s| {
        s.flex_col()
            .width(theme::CONVERSATION_WIDTH)
            .max_width_full()
    });
    let column_id = column.id();
    let place = Rc::new(Place::default());
    let seen = place.clone();
    let shift = RwSignal::new(0.0_f64);

    let page = Container::new(column)
        .style(|s| {
            s.width_full()
                .items_start()
                .justify_center()
                .padding(theme::CONVERSATION_PADDING)
        })
        .scroll()
        // Where the reader left the page, put back as it is built, since a
        // file's tab showing in its place takes the page away; and kept as it
        // scrolls. The request waits for the page to be laid out.
        .scroll_to(move || {
            let y = state.conversation_scroll.get_untracked();
            (y > 0.0).then_some(floem::kurbo::Point::new(0.0, y))
        })
        // Page Up and Page Down: a page, less a line kept in sight so the
        // reader keeps their place. Cmd-Up and Cmd-Down: the top, or the end,
        // by a delta past it, which the scroll clamps. A delta rather than a
        // point shows the bars that hide while idle, as the wheel does.
        .scroll_delta(move || {
            let Some(paging) = state.paging.get() else {
                return floem::kurbo::Vec2::ZERO;
            };
            state.paging.set(None);
            crate::review::notified::cancel(state);
            let page = || {
                let height = state
                    .conversation_page
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
        // What keeps the reader's place: a delta rather than a point, sent at
        // once rather than after layout, so a view that brings itself into
        // view from the same layout, as a composer opening does, moves the
        // page after it, from where it has been put back to.
        .scroll_delta(move || floem::kurbo::Vec2::new(0.0, shift.get()))
        .on_event_cont(
            ScrollChanged::listener(),
            move |_, event: &ScrollChanged| {
                state.conversation_scroll.set(event.offset.y);
                if let Some(page) = state.conversation_page.get_untracked() {
                    seen.see(
                        &Place::rows(column_id, timeline_id),
                        event.offset.y,
                        page.get_content_rect_local().height(),
                    );
                }
            },
        )
        .on_event_cont(el::PointerWheel, move |_, _| {
            crate::review::notified::cancel(state)
        })
        .debug_name("conversation")
        // A bare key goes to the focused view and bubbles up from there, and
        // a click on the page would otherwise focus nothing: the column takes
        // the focus so `a` reaches the window's handler.
        .style(|s| {
            s.size_full()
                .min_height(0.0)
                .background(theme::canvas())
                .focusable()
        });

    // Cmd-F's bar looks through the page and scrolls it; its matches
    // follow the query from here, and the page's text as it is laid out.
    let page_id = page.id();
    state.conversation_page.set(Some(page_id));
    conversation_find::wire(state);
    // The page takes the keyboard when it comes up with nothing else
    // holding it, as when it replaces the code and the editor goes with
    // the focus: floem hands a bare key, Page Down or `a`, only to what
    // has the focus, and with nothing focused it goes nowhere. Not from a
    // composer, which takes the keyboard itself as it opens, nor from the
    // find bar; and not from the sidebar's list, or whatever else the
    // reader left it with. Asked from the first layout, as a request made
    // while the page is being built is dropped.
    let asked = Rc::new(std::cell::Cell::new(false));
    // And back to the page when a composer on it closes.
    floem::reactive::Effect::new(move |was_open: Option<bool>| {
        let open = state.composer.with(Option::is_some);
        if was_open == Some(true) && !open {
            page_id.request_focus();
        }
        open
    });
    Stack::new((page, find::bar(state, find::Pane::Conversation)))
        .on_event_cont(el::UpdatePhaseLayout, move |cx, _| {
            // First, so the page is back in place before what the find
            // bar or a view laid out here scrolls to.
            let rows = Place::rows(column_id, timeline_id);
            let offset = place.offset.get();
            let offset = match place.moved(&rows) {
                Some(moved) => {
                    shift.set(moved);
                    (offset + moved).max(0.0)
                }
                None => offset,
            };
            place.see(&rows, offset, page_id.get_content_rect_local().height());
            if !asked.replace(true)
                && state.composer.with_untracked(Option::is_none)
                && !state.find_focused.get_untracked()
                && crate::ui::focused_descendant(cx, page_id.root()).is_none()
            {
                page_id.request_focus();
            }
            conversation_find::laid_out(state)
        })
        .on_cleanup(move || {
            if state.conversation_page.get_untracked() == Some(page_id) {
                state.conversation_page.set(None);
            }
        })
        .style(|s| s.size_full().min_height(0.0).position(Position::Relative))
}

/// The page's cards, oldest first as `review::timeline` sorts them, the
/// description at their head. The page shows them the other way round.
/// Tracked.
pub(crate) fn cards(state: ReviewState) -> Vec<Card> {
    let pull = state.pull_request.get();
    let discussion = state.discussion.get();
    let issue = state.issue.get();
    let comments = state.issue_comments.get();
    let issue_events = state.issue_events.get();
    // Only a bare branch's own commits: an item about a pull request
    // or an issue has GitHub's timeline, which may not have landed yet.
    let bare = state
        .item
        .with(|item| item.as_ref().is_some_and(|item| item.subject.is_none()));
    let commits = if bare {
        state.commits.get()
    } else {
        Default::default()
    };
    state.comment_threads.with(|threads| {
        timeline(
            pull.as_deref(),
            discussion.as_deref(),
            issue
                .as_deref()
                .map(|i| (i, comments.as_slice(), issue_events.as_slice())),
            &commits,
            threads.iter().filter(|thread| state.thread_visible(thread)),
        )
    })
}

/// The threads of fixed and dismissed findings, which the timeline leaves
/// out, as one row under it that shows them, and once they are shown,
/// hides them again. Gone when there are none, and on a past review,
/// which shows every finding it had.
fn resolved_row(app: AppState, state: ReviewState, older: RwSignal<bool>) -> impl IntoView {
    // How many of the item's threads are left out now, and how many the
    // row would leave out: read off the threads themselves, not the
    // filter, so the row says what the page shows. A thread with nothing
    // in it is never shown, and one with a draft always is, so neither
    // is counted.
    let counts = Memo::new(move |_| {
        if state.historical_review.with(Option::is_some) {
            return (0, 0);
        }
        let Some(item_id) = state
            .item
            .with(|item| item.as_ref().map(|item| item.id.clone()))
        else {
            return (0, 0);
        };
        let resolved = app
            .review_findings_for(&item_id)
            .iter()
            .filter(|finding| !finding.is_actionable())
            .map(|finding| finding.finding.thread_id.clone())
            .collect::<std::collections::HashSet<_>>();
        state.hidden_review_threads.with(|hidden| {
            state.comment_threads.with(|threads| {
                let movable = threads.iter().filter(|thread| {
                    !thread.messages.is_empty() && !thread.messages.iter().any(|m| m.is_draft())
                });
                let (mut out, mut settled) = (0, 0);
                for thread in movable {
                    out += usize::from(hidden.contains(&thread.id));
                    settled += usize::from(resolved.contains(&thread.id));
                }
                (out, settled)
            })
        })
    });
    let rule = || {
        Empty::new().style(|s| {
            s.height(1.0)
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .background(theme::border_muted())
        })
    };
    Stack::horizontal((
        rule(),
        cards::text_action(
            move || {
                let (out, settled) = counts.get();
                let (verb, n) = if out > 0 {
                    ("Show", out)
                } else {
                    ("Hide", settled)
                };
                let noun = if n == 1 { "finding" } else { "findings" };
                format!("{verb} {n} resolved {noun}")
            },
            "show-resolved",
            move || {
                let Some(item_id) = state
                    .item
                    .with_untracked(|item| item.as_ref().map(|item| item.id.clone()))
                else {
                    return;
                };
                let findings = app.review_findings_for(&item_id);
                if counts.get_untracked().0 > 0 {
                    state.apply_review_finding_filter(ReviewFindingFilter::All, &findings);
                    // A resolved thread is often an old one, which would
                    // come back among the folded cards and show nothing.
                    older.set(true);
                } else {
                    state.apply_review_finding_filter(ReviewFindingFilter::Open, &findings);
                }
            },
        ),
        rule(),
    ))
    .debug_name("resolved findings")
    .style(move |s| {
        let (out, settled) = counts.get();
        s.items_center()
            .width_full()
            .gap(8.0)
            .margin_top(theme::CARD_GAP)
            .apply_if(out == 0 && settled == 0, |s| s.hide())
    })
}

/// One card, with the room above it.
fn card_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    card: Card,
    gap: impl Fn() -> f64 + 'static,
    unfolded: Unfolded,
) -> floem::AnyView {
    let pull = state.pull_request.get_untracked();
    let debug = card.kind();
    let card_id = cards::card_id(&card);
    let view: floem::AnyView = match card {
        // What Perch started from an issue is the summary's In Perch row,
        // not repeated under the description.
        Card::Description {
            body,
            node_id,
            editable,
            ..
        } => cards::description_card(
            app,
            state,
            backend,
            body,
            node_id,
            editable,
            unfolded.description,
        )
        .into_any(),
        Card::Comment(comment) => cards::comment_card(app, state, backend, comment).into_any(),
        Card::Review { review } => cards::review_card(app, state, backend, review).into_any(),
        Card::Event(event) => {
            let link = state.item.with_untracked(|i| {
                let item = i.as_deref()?;
                Some((
                    item.subject.as_ref()?.number(),
                    crate::ui::items::github_url(item)?,
                ))
            });
            cards::event_row(
                app,
                state,
                event,
                pull.map(|p| p.author_login.clone()),
                link,
            )
            .into_any()
        }
        Card::Thread(thread) => {
            cards::thread_card(app, state, backend, open_file, thread).into_any()
        }
    };
    // Asked for from elsewhere, a question's run in the Agents dropdown or
    // an unread notification's open, the card comes into view as it
    // flashes. The first run only subscribes, as the flash's does.
    let id = view.view_id();
    let focus_id = card_id.clone();
    floem::reactive::Effect::new(move |first: Option<()>| {
        let focused = state.focus_card.with(|f| f.as_ref() == Some(&focus_id));
        if first.is_some() && focused {
            id.scroll_to(None);
        }
    });
    let flashing = RwSignal::new(false);
    threads::wire_flash(state, card_id.clone(), flashing);
    // Built for the asking, as a finding's thread the filter hid is, or
    // asked for before it was laid out, it takes the focus once it has
    // somewhere to scroll to.
    let view = view
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if state
                .pending_focus_card
                .with_untracked(|p| p.as_ref() == Some(&card_id))
            {
                state.pending_focus_card.set(None);
                state.focus_card.set(Some(card_id.clone()));
            }
        })
        .into_any();
    view.debug_name(format!("card:{debug}"))
        .style(move |s| {
            s.width_full()
                .margin_top(gap())
                .apply_if(flashing.get(), |s| s.border_color(theme::accent()))
        })
        .into_any()
}

/// The composer for asking about the item as a whole, in place under the
/// description and over the timeline, where what it starts will land; or
/// the box that opens it. A follow-up's composer is under its thread
/// instead, and one editing a comment is in its card.
fn composer(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // Keyed on whether one is open, what it is about and which opening it
    // is, not on the composer itself: picking a model writes the composer,
    // and a rebuild on that would throw the typed question away. The
    // opening count is there so Ask about this on a second comment, which
    // opens a composer about the same thing with another quote, gets a
    // fresh box rather than the first one's text.
    let open = floem::reactive::Memo::new(move |_| {
        state.composer.with(|c| {
            c.as_ref()
                .filter(|c| c.thread_id.is_none() && c.editing.is_none())
                .map(|c| (c.anchor.clone(), c.opening))
        })
    });
    dyn_container(
        move || open.get(),
        move |open| match open {
            Some(_) => match state.composer.get_untracked() {
                Some(c) => threads::composer_view(app, state, backend.clone(), c, ThreadFace::Card)
                    .into_any(),
                None => Empty::new().into_any(),
            },
            None => cards::ask_box(app, state).into_any(),
        },
    )
    .debug_name("composer")
    .style(|s| s.width_full())
}

/// The whole window with an item open on Conversation, and what is on it.
#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_github::{
        Anchor, Author, CommentMessage, CommentThread, Discussion, IssueComment, Item,
        ItemCheckout, ItemSubject, PullEvent, PullEventKind, PullRequest, Review, ReviewComment,
        RunRecord, RunStatus, RunStep, Side, ThreadOrigin,
    };

    use crate::review::model::{Paging, ReviewState};
    use crate::review::threads::ThreadFace;
    use crate::state::AppState;

    fn at(s: &str) -> jiff::Timestamp {
        s.parse().unwrap()
    }

    fn pull() -> PullRequest {
        PullRequest {
            number: 57,
            title: "Fence the manifest".into(),
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "fence".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: true,
            state: "closed".into(),
            additions: 12,
            deletions: 3,
            changed_files: 1,
            repo_full_name: "criccomini/perch".into(),
            html_url: "https://github.com/criccomini/perch/pull/57".into(),
            head_repo_full_name: None,
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: Some(at("2026-09-17T15:00:00Z")),
            merged_by: Some("rohan".into()),
            closed_at: Some(at("2026-09-17T15:00:00Z")),
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        }
    }

    /// One comment, one approving review with a line comment and a reply,
    /// and the merge.
    fn discussion() -> Discussion {
        let line = |id: u64, reply_to: Option<u64>, when: &str, body: &str| ReviewComment {
            node_id: String::new(),
            reactions: Default::default(),
            id,
            pull_request_review_id: Some(9),
            in_reply_to_id: reply_to,
            user: "chris".into(),
            body: body.into(),
            path: "src/manifest.rs".into(),
            line: Some(41),
            original_line: None,
            start_line: None,
            original_start_line: None,
            start_side: None,
            commit_id: String::new(),
            original_commit_id: String::new(),
            user_avatar_url: String::new(),
            diff_hunk: "@@ -39,3 +39,3 @@\n a\n-b\n+c".into(),
            side: Side::Right,
            created_at: at(when),
            updated_at: None,
            html_url: format!("https://github.com/criccomini/perch/pull/57#discussion_r{id}"),
            viewer_can_delete: false,
            viewer_can_update: false,
        };
        Discussion {
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            comments: vec![IssueComment {
                node_id: String::new(),
                reactions: Default::default(),
                id: 1,
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "Thanks".into(),
                created_at: at("2026-09-15T10:00:00Z"),
                updated_at: None,
                html_url: "https://github.com/criccomini/perch/pull/57#issuecomment-1".into(),
                viewer_can_delete: false,
                viewer_can_update: false,
            }],
            reviews: vec![Review {
                viewer_can_update: false,
                node_id: String::new(),
                reactions: Default::default(),
                id: 9,
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "Looks right.".into(),
                state: "APPROVED".into(),
                submitted_at: Some(at("2026-09-16T11:40:00Z")),
                html_url: String::new(),
            }],
            review_comments: vec![
                line(31, None, "2026-09-16T11:38:00Z", "epoch"),
                line(32, Some(31), "2026-09-16T12:00:00Z", "the reply"),
            ],
            events: vec![
                PullEvent::new(PullEventKind::Opened, at("2026-09-14T14:02:00Z"), "rohan"),
                PullEvent::new(PullEventKind::Merged, at("2026-09-17T15:00:00Z"), "rohan"),
            ],
            review_threads: Vec::new(),
        }
    }

    fn run() -> RunRecord {
        RunRecord {
            id: "r1".into(),
            owner: perch_github::RunOwner::Discussion {
                item_id: "wt".into(),
                thread_id: "t1".into(),
                request_id: "q1".into(),
            },
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "high".into(),
            session_id: Some("sess".into()),
            status: RunStatus::Completed,
            cost_usd: Some(0.03),
            duration_ms: Some(14_000),
            steps: vec![
                RunStep {
                    summary: "Read src/manifest.rs".into(),
                    ok: true,
                    note: false,
                    pending: false,
                    user: false,
                },
                RunStep {
                    summary: "Ran git log".into(),
                    ok: true,
                    note: false,
                    pending: false,
                    user: false,
                },
            ],
            error: None,
            started_at: Some(at("2026-09-16T13:00:00Z")),
            ended_at: Some(at("2026-09-16T13:00:14Z")),
            ..RunRecord::default()
        }
    }

    /// A thread asked between the review and the merge.
    fn thread() -> CommentThread {
        let message = |id: &str, author: Author, body: &str, when: &str| CommentMessage {
            id: id.into(),
            thread_id: "t1".into(),
            origin: ThreadOrigin::Local,
            author,
            body: body.into(),
            created_at: at(when),
            run: Some(run()),
            github_id: None,
            node_id: None,
            html_url: None,
            diff_hunk: None,
            reactions: Default::default(),
        };
        CommentThread {
            id: "t1".into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            github_id: None,
            anchor: Some(Anchor {
                path: "src/manifest.rs".into(),
                start_line: 41,
                line: 58,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: at("2026-09-16T13:00:00Z"),
            messages: vec![
                message("m1", Author::User, "why the fence?", "2026-09-16T13:00:00Z"),
                message(
                    "m2",
                    Author::Agent {
                        backend: "claude-code".into(),
                        model: "sonnet".into(),
                    },
                    "because",
                    "2026-09-16T13:00:14Z",
                ),
            ],
        }
    }

    /// Conversation, with its timeline and a thread, all of it dark once the system is.
    #[test]
    fn goes_dark_with_the_system() {
        let mut harness = app_harness().0;
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(
            left.is_empty(),
            "light colours left in dark:\n{}",
            left.join("\n")
        );
    }

    fn app_harness() -> (HeadlessHarness, AppState, ReviewState) {
        app_harness_on(crate::backend::Backend::detached(), discussion())
    }

    /// The window with the pull request open on Conversation, with this
    /// discussion, and the backend the window's asks go to.
    fn app_harness_on(
        backend: crate::backend::Backend,
        discussion: Discussion,
    ) -> (HeadlessHarness, AppState, ReviewState) {
        app_harness_prepared(backend, discussion, |_| {})
    }

    /// The same, with the review state set up further by `prepare` before
    /// the window is built.
    fn app_harness_prepared(
        backend: crate::backend::Backend,
        discussion: Discussion,
        prepare: impl FnOnce(ReviewState),
    ) -> (HeadlessHarness, AppState, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let pr = pull();
        let now = at("2026-09-18T09:59:00Z");
        let mut item = Item::new("criccomini/perch", now);
        item.id = "wt".into();
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        item.checkout = Some(ItemCheckout {
            branch: "perch/pr-57".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: std::path::PathBuf::from("/tmp/wt"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(crate::review::model::Phase::Ready);
        state.pull_request.set(Some(Arc::new(pr)));
        state.install_reactions(discussion.reactions_by_id());
        state.discussion.set(Some(Arc::new(discussion)));
        state.comment_threads.set(Arc::new(vec![thread()]));
        // See the field: a focused editor cannot be driven headlessly.
        state.composer_autofocus.set(false);
        prepare(state);
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
    }

    /// The reactions on a comment are pills under it, one a kind, and a
    /// click on one toggles it where it is and asks the backend; a second
    /// click waits for the answer. An entry nobody reacted to has no row,
    /// and one an older build kept without a node id has no picker.
    #[test]
    fn a_reaction_pill_toggles_where_it_is_and_asks_the_backend() {
        use perch_github::{ReactionGroup, ReactionKind, Reactions};
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut discussion = discussion();
        discussion.node_id = "PR_57".into();
        discussion.comments[0].node_id = "IC_1".into();
        discussion.comments[0].reactions = Reactions {
            groups: vec![
                ReactionGroup {
                    kind: ReactionKind::ThumbsUp,
                    count: 2,
                    viewer_reacted: false,
                    reactors: vec![],
                },
                ReactionGroup {
                    kind: ReactionKind::Rocket,
                    count: 1,
                    viewer_reacted: true,
                    reactors: vec![],
                },
            ],
        };
        let (mut harness, app, state) = app_harness_on(backend, discussion);
        let root = harness.root_id();
        let comment = find(root, "card:comment").unwrap();
        let description = find(root, "card:description").unwrap();
        let review = find(root, "card:review").unwrap();
        to_top(&mut harness, comment);

        // The pills, and the picker at their end; the description, with
        // nothing on it, has the row hidden; the review, kept without a
        // node id, has no picker at all.
        let pill = |harness: &HeadlessHarness, kind: &str| {
            let id = find(comment, &format!("reaction:{kind}"))
                .unwrap_or_else(|| panic!("no {kind} pill"));
            let mut all = Vec::new();
            names(id, &mut all);
            (id, all.join(" "), harness.get_layout_rect(id).height())
        };
        let (thumbs, text, height) = pill(&harness, "THUMBS_UP");
        assert!(text.contains("\"2\""), "{text}");
        assert_eq!(height, crate::theme::REACTION_PILL_HEIGHT);
        assert!(pill(&harness, "ROCKET").1.contains("\"1\""));
        assert!(find(comment, "reaction:EYES").is_none(), "nobody left one");
        assert!(find(comment, "react-button").is_some());
        let row = find(description, "reactions").unwrap();
        assert_eq!(
            harness.get_layout_rect(row).height(),
            0.0,
            "nothing to show"
        );
        assert!(
            find(description, "react-button").is_some(),
            "the picker is the way in"
        );
        assert!(find(review, "react-button").is_none());
        assert!(find(review, "reactions").is_none());

        // A click: the pill counts the reader in before GitHub is asked.
        let c = thumbs.get_visual_rect_no_clip().center();
        assert_eq!(
            harness.element_id_at(c.x, c.y).map(|e| e.owning_id()),
            Some(thumbs),
            "the pill is not under the pointer"
        );
        // Whatever the window asked for on its way up is not the click's.
        while commands.try_recv().is_ok() {}
        harness.click(c.x, c.y);
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::SetReaction { item_id, node_id, kind: ReactionKind::ThumbsUp, add: true })
                if item_id == "wt" && node_id == "IC_1"
        ));
        assert!(state
            .reactions_of("IC_1")
            .viewer_reacted(ReactionKind::ThumbsUp));
        harness.rebuild();
        harness.paint();
        let (thumbs, text, _) = pill(&harness, "THUMBS_UP");
        assert!(text.contains("\"3\""), "{text}");

        // A second click on the same pill waits for GitHub.
        let c = thumbs.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        assert!(commands.try_recv().is_err(), "asked twice");
        assert_eq!(state.reactions_of("IC_1").count(ReactionKind::ThumbsUp), 3);

        // GitHub's answer is what the row shows then, and the pill is a
        // pill again.
        app.apply_with_review(
            crate::backend::Event::ReactionsUpdated {
                item_id: "wt".into(),
                node_id: "IC_1".into(),
                kind: ReactionKind::ThumbsUp,
                reactions: Reactions {
                    groups: vec![ReactionGroup {
                        kind: ReactionKind::ThumbsUp,
                        count: 4,
                        viewer_reacted: true,
                        reactors: vec![],
                    }],
                },
            },
            state,
        );
        harness.rebuild();
        harness.paint();
        let (thumbs, text, _) = pill(&harness, "THUMBS_UP");
        assert!(text.contains("\"4\""), "{text}");
        assert!(
            find(comment, "reaction:ROCKET").is_none(),
            "GitHub says nobody's"
        );
        let c = thumbs.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::SetReaction {
                kind: ReactionKind::ThumbsUp,
                add: false,
                ..
            })
        ));
        assert_eq!(state.reactions_of("IC_1").count(ReactionKind::ThumbsUp), 3);
    }

    /// A comment with a Thumbs up from others and the reader's Rocket, in
    /// the window, with the backend it asks.
    fn reacted_harness() -> (
        HeadlessHarness,
        ReviewState,
        tokio::sync::mpsc::UnboundedReceiver<crate::backend::Command>,
    ) {
        use perch_github::{ReactionGroup, ReactionKind, Reactions};
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut discussion = discussion();
        discussion.comments[0].node_id = "IC_1".into();
        discussion.comments[0].reactions = Reactions {
            groups: vec![
                ReactionGroup {
                    kind: ReactionKind::ThumbsUp,
                    count: 2,
                    viewer_reacted: false,
                    reactors: vec![],
                },
                ReactionGroup {
                    kind: ReactionKind::Rocket,
                    count: 1,
                    viewer_reacted: true,
                    reactors: vec![],
                },
            ],
        };
        let (mut harness, _, state) = app_harness_on(backend, discussion);
        let comment = find(harness.root_id(), "card:comment").unwrap();
        to_top(&mut harness, comment);
        while commands.try_recv().is_ok() {}
        (harness, state, commands)
    }

    /// The page scrolled to put `id` near the top of the window, or as
    /// near as the page's end lets it, as a reader scrolls to what they
    /// are reading: the older cards are under the fold.
    fn to_top(harness: &mut HeadlessHarness, id: floem::ViewId) {
        let viewport = find(harness.root_id(), "conversation")
            .unwrap()
            .get_visual_rect_no_clip();
        let by = id.get_visual_rect_no_clip().y0 - viewport.y0 - 40.0;
        let c = viewport.center();
        if by > 0.0 {
            harness.scroll_down(c.x, c.y, by);
        } else if by < 0.0 {
            harness.scroll_up(c.x, c.y, -by);
        }
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
    }

    /// Show older, pressed.
    fn show_older(harness: &mut HeadlessHarness) {
        let button = find(harness.root_id(), "show-older").expect("nothing is folded");
        to_top(harness, button);
        click_on(harness, button);
    }

    fn click_on(harness: &mut HeadlessHarness, id: floem::ViewId) {
        let c = id.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        harness.rebuild();
        harness.paint();
    }

    /// A named key going down, or, with `up`, coming up.
    fn key(
        code: floem::prelude::Code,
        named: floem::prelude::NamedKey,
        modifiers: floem::prelude::Modifiers,
        up: bool,
    ) -> floem::event::Event {
        use floem::prelude::{Key, KeyState, KeyboardEvent};
        floem::event::Event::Key(KeyboardEvent {
            state: if up { KeyState::Up } else { KeyState::Down },
            key: Key::Named(named),
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    fn escape() -> floem::event::Event {
        use floem::prelude::{Code, Modifiers, NamedKey};
        key(Code::Escape, NamedKey::Escape, Modifiers::empty(), false)
    }

    /// The face after the pills opens GitHub's picker over itself: a card
    /// of the eight in a row, starting at the face's left edge, drawn
    /// over everything under it. A pick asks the backend, takes the
    /// reader's own back, and closes the picker.
    #[test]
    fn the_rows_face_opens_the_picker_over_it_and_a_pick_closes_it() {
        use perch_github::ReactionKind;
        let (mut harness, state, mut commands) = reacted_harness();
        let comment = find(harness.root_id(), "card:comment").unwrap();
        let row = find(comment, "reactions").unwrap();
        let face = find(row, "react-button").unwrap();
        assert_eq!(
            harness.get_layout_rect(face).height(),
            crate::theme::REACTION_PILL_HEIGHT,
            "the face is a pill's height"
        );
        assert!(find(harness.root_id(), "react-picker").is_none());

        click_on(&mut harness, face);
        let picker = find(harness.root_id(), "react-picker").expect("the click opened it");
        let card = picker.get_visual_rect_no_clip();
        let at = face.get_visual_rect_no_clip();
        assert_eq!((card.width(), card.height()), (294.0, 42.0));
        assert_eq!(card.y1, at.y0 - 8.0, "not over the face");
        assert_eq!(card.x0, at.x0, "not from the face's left edge");
        for (i, kind) in ReactionKind::ALL.into_iter().enumerate() {
            let pick = find(picker, &format!("react-pick:{}", kind.graphql()))
                .unwrap_or_else(|| panic!("no {kind:?}"));
            let rect = pick.get_visual_rect_no_clip();
            assert_eq!((rect.width(), rect.height()), (32.0, 32.0));
            assert_eq!(
                rect.x0,
                card.x0 + 5.0 + 36.0 * i as f64,
                "{kind:?} out of line"
            );
            let c = rect.center();
            assert_eq!(
                harness.element_id_at(c.x, c.y).map(|e| e.owning_id()),
                Some(pick),
                "something is drawn over {kind:?}"
            );
        }
        let order = harness.paint_and_get_order();
        let painted = |id| order.iter().position(|&p| p == id).expect("not painted");
        assert!(
            painted(picker) > painted(comment),
            "the entry is painted over the picker"
        );
        assert!(
            harness.is_focused(find(picker, "react-pick:THUMBS_UP").unwrap()),
            "the first emoji did not take the focus"
        );

        // The reader's Rocket is taken back, and the picker goes.
        let rocket = find(picker, "react-pick:ROCKET").unwrap();
        click_on(&mut harness, rocket);
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::SetReaction {
                kind: ReactionKind::Rocket,
                add: false,
                ..
            })
        ));
        assert!(!state
            .reactions_of("IC_1")
            .viewer_reacted(ReactionKind::Rocket));
        assert!(
            find(harness.root_id(), "react-picker").is_none(),
            "the pick left it open"
        );

        // An Eyes is left, and a pill is drawn for it.
        click_on(&mut harness, face);
        let eyes = find(harness.root_id(), "react-pick:EYES").unwrap();
        click_on(&mut harness, eyes);
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::SetReaction {
                kind: ReactionKind::Eyes,
                add: true,
                ..
            })
        ));
        assert!(find(comment, "reaction:EYES").is_some());
    }

    /// The byline's face, drawn while the entry is under the pointer,
    /// opens the picker under itself and running left, and it stays
    /// drawn, as the picker's anchor, while the picker is open.
    #[test]
    fn the_bylines_face_opens_the_picker_under_it() {
        let (mut harness, _, _commands) = reacted_harness();
        let comment = find(harness.root_id(), "card:comment").unwrap();
        let face = find(comment, "react-button").unwrap();
        assert_eq!(
            harness.get_layout_rect(face).height(),
            0.0,
            "drawn without the pointer"
        );
        let over = comment.get_visual_rect_no_clip().center();
        harness.pointer_move(over.x, over.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(harness.get_layout_rect(face).height(), 24.0);

        click_on(&mut harness, face);
        let card = find(harness.root_id(), "react-picker")
            .expect("the click opened it")
            .get_visual_rect_no_clip();
        let at = face.get_visual_rect_no_clip();
        assert_eq!(card.y0, at.y1 + 8.0, "not under the face");
        assert_eq!(card.x1, at.x1, "not from the face's right edge");

        // The pointer leaves the entry for the picker's backdrop; the face
        // stays, and the picker with it.
        harness.pointer_move(5.0, 5.0);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            harness.get_layout_rect(face).height(),
            24.0,
            "the anchor went"
        );
        let picker = find(harness.root_id(), "react-picker").expect("the picker went");
        assert_eq!(picker.get_visual_rect_no_clip(), card);
    }

    /// The picker hangs from its face, so an entry above that grows while
    /// it is open carries it down with the face. At the page's top, as
    /// further down the page keeps what is in view where it was.
    #[test]
    fn the_picker_follows_its_face_when_the_column_moves() {
        let (mut harness, state, _commands) = reacted_harness();
        harness.set_size(1400.0, 2400.0);
        state.paging.set(Some(crate::review::model::Paging::Top));
        settle(&mut harness);
        assert_eq!(state.conversation_scroll.get_untracked(), 0.0);
        let comment = find(harness.root_id(), "card:comment").unwrap();
        let face = find(find(comment, "reactions").unwrap(), "react-button").unwrap();
        click_on(&mut harness, face);
        let before = face.get_visual_rect_no_clip();

        let mut longer = state.discussion.get_untracked().unwrap().as_ref().clone();
        longer.body = "A much longer description.\n\n".repeat(6);
        state.discussion.set(Some(std::sync::Arc::new(longer)));
        harness.rebuild();
        harness.paint();

        let at = face.get_visual_rect_no_clip();
        assert!(
            at.y0 > before.y0 + 50.0,
            "the face did not move: {before:?} to {at:?}"
        );
        let card = find(harness.root_id(), "react-picker")
            .expect("the picker closed")
            .get_visual_rect_no_clip();
        assert_eq!(
            (card.y1, card.x0),
            (at.y0 - 8.0, at.x0),
            "the picker stayed behind"
        );
    }

    /// A click anywhere but the picker closes it and does nothing else:
    /// the pill under it is not toggled. So does Escape. Either way the
    /// face has the focus after, so a key still reaches the column.
    #[test]
    fn a_click_elsewhere_or_escape_closes_the_picker_and_does_nothing_else() {
        use perch_github::ReactionKind;
        let (mut harness, state, mut commands) = reacted_harness();
        let comment = find(harness.root_id(), "card:comment").unwrap();
        let row = find(comment, "reactions").unwrap();
        let face = find(row, "react-button").unwrap();
        let thumbs = find(comment, "reaction:THUMBS_UP").unwrap();

        click_on(&mut harness, face);
        assert!(find(harness.root_id(), "react-picker").is_some());
        click_on(&mut harness, thumbs);
        assert!(
            find(harness.root_id(), "react-picker").is_none(),
            "the click left it open"
        );
        assert!(commands.try_recv().is_err(), "the click reached the pill");
        assert_eq!(state.reactions_of("IC_1").count(ReactionKind::ThumbsUp), 2);
        assert!(harness.is_focused(face), "the click left the focus nowhere");

        // The face again, which a click while it is open closes.
        click_on(&mut harness, face);
        click_on(&mut harness, face);
        assert!(
            find(harness.root_id(), "react-picker").is_none(),
            "the face left it open"
        );

        // Escape reaches the picker, which took the focus as it opened.
        click_on(&mut harness, face);
        assert!(!harness.is_focused(face));
        harness.dispatch_event(escape());
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "react-picker").is_none(),
            "Escape left it open"
        );
        assert!(
            harness.is_focused(face),
            "the face did not take the focus back"
        );
        assert!(commands.try_recv().is_err());
    }

    /// The keys move the focus along the picker's row and around from
    /// either end, Tab included, so the focus stays in it; Return picks.
    #[test]
    fn the_keys_move_along_the_picker_and_return_picks() {
        use floem::prelude::{Code, Modifiers, NamedKey};
        use perch_github::ReactionKind;
        let (mut harness, _, mut commands) = reacted_harness();
        let comment = find(harness.root_id(), "card:comment").unwrap();
        let face = find(find(comment, "reactions").unwrap(), "react-button").unwrap();
        click_on(&mut harness, face);
        let picker = find(harness.root_id(), "react-picker").unwrap();
        let pick = |kind: &str| find(picker, &format!("react-pick:{kind}")).unwrap();
        let none = Modifiers::empty();
        for (code, named, modifiers, then) in [
            (Code::ArrowLeft, NamedKey::ArrowLeft, none, "EYES"),
            (Code::ArrowRight, NamedKey::ArrowRight, none, "THUMBS_UP"),
            (Code::Tab, NamedKey::Tab, none, "THUMBS_DOWN"),
            (Code::Tab, NamedKey::Tab, Modifiers::SHIFT, "THUMBS_UP"),
            (Code::Tab, NamedKey::Tab, Modifiers::SHIFT, "EYES"),
            (Code::Tab, NamedKey::Tab, none, "THUMBS_UP"),
            (Code::End, NamedKey::End, none, "EYES"),
            (Code::Home, NamedKey::Home, none, "THUMBS_UP"),
            (Code::ArrowRight, NamedKey::ArrowRight, none, "THUMBS_DOWN"),
            (Code::ArrowRight, NamedKey::ArrowRight, none, "LAUGH"),
        ] {
            harness.dispatch_event(key(code, named, modifiers, false));
            harness.rebuild();
            harness.paint();
            assert!(
                harness.is_focused(pick(then)),
                "{code:?} did not reach {then}"
            );
        }

        harness.dispatch_event(key(Code::Enter, NamedKey::Enter, none, false));
        harness.dispatch_event(key(Code::Enter, NamedKey::Enter, none, true));
        harness.rebuild();
        harness.paint();
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::SetReaction {
                kind: ReactionKind::Laugh,
                add: true,
                ..
            })
        ));
        assert!(
            find(harness.root_id(), "react-picker").is_none(),
            "Return left it open"
        );
        assert!(harness.is_focused(face));
    }

    /// Every view's debug names, in tree order.
    fn names(id: floem::ViewId, out: &mut Vec<String>) {
        out.push(id.debug_name());
        for child in id.children() {
            names(child, out);
        }
    }

    /// The first view whose debug name contains `text`.
    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, text))
    }

    /// Where `needle` is drawn on an event's line, in the window: the
    /// line's linked text, and the box around the words.
    fn on_event_line(root: floem::ViewId, needle: &str) -> (floem::ViewId, floem::kurbo::Rect) {
        use crate::review::linked_text::{range_rect, shown_under, with_text};
        let mut rows = Vec::new();
        fn rows_under(id: floem::ViewId, out: &mut Vec<floem::ViewId>) {
            if id.debug_name().contains("card:event") {
                out.push(id);
            }
            for child in id.children() {
                rows_under(child, out);
            }
        }
        rows_under(root, &mut rows);
        for line in rows.into_iter().flat_map(shown_under) {
            let Some(Some(start)) = with_text(line, |text| text.find(needle)) else {
                continue;
            };
            let rect = range_rect(line, start..start + needle.len()).expect("laid out");
            return (
                line,
                rect + line.get_visual_rect_no_clip().origin().to_vec2(),
            );
        }
        panic!("no event's line has {needle:?}");
    }

    /// Cmd-C, and what it copied.
    fn copy(harness: &mut HeadlessHarness) -> Vec<String> {
        use floem::event::Event;
        use floem::ui_events::keyboard::{Code, Key, KeyState, KeyboardEvent};
        harness.dispatch_event(Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Character("c".into()),
            code: Code::KeyC,
            location: Default::default(),
            modifiers: crate::ui::OS_MOD,
            repeat: false,
            is_composing: false,
        }));
        crate::ui::widgets::copied()
    }

    /// Whether a view is laid out at all: a hidden one has no size.
    fn shown(harness: &HeadlessHarness, text: &str) -> bool {
        find(harness.root_id(), text).is_some_and(|id| harness.get_layout_rect(id).height() > 0.0)
    }

    /// The cards on the page, the description first, as `cards_in_order`
    /// names them.
    fn card_views(harness: &HeadlessHarness) -> Vec<floem::ViewId> {
        fn under(id: floem::ViewId, out: &mut Vec<floem::ViewId>) {
            if id
                .debug_name()
                .split_whitespace()
                .any(|w| w.starts_with("card:"))
            {
                out.push(id);
            }
            for child in id.children() {
                under(child, out);
            }
        }
        let mut found = Vec::new();
        under(harness.root_id(), &mut found);
        found
    }

    fn cards_in_order(harness: &HeadlessHarness) -> Vec<String> {
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        all.into_iter()
            .filter_map(|n| {
                n.split_whitespace()
                    .find(|w| w.starts_with("card:"))
                    .map(str::to_string)
            })
            .collect()
    }

    /// Every view under `id` whose debug name contains `text`, in tree
    /// order.
    fn all_named(id: floem::ViewId, text: &str) -> Vec<floem::ViewId> {
        let mut found = Vec::new();
        fn under(id: floem::ViewId, text: &str, out: &mut Vec<floem::ViewId>) {
            if id.debug_name().contains(text) {
                out.push(id);
            }
            for child in id.children() {
                under(child, text, out);
            }
        }
        under(id, text, &mut found);
        found
    }

    /// The page from its top: the description, with the summary's line
    /// as its strip, the composer, and the timeline under them, the newest card first and
    /// the oldest last. The title is the window bar's alone, and
    /// the item's runs, even with one there, are the bar's Agents
    /// dropdown's.
    #[test]
    fn the_page_runs_from_the_summary_down_to_the_oldest_card() {
        let (mut harness, app, _) = app_harness();
        app.merge_agent_runs([perch_github::RunSummary::from(&run())]);
        let mut review = crate::ui::agents::model::tests::review_summary("group-1", &[]);
        review.group.item_id = "wt".into();
        app.review_groups.update(|all| {
            all.insert("wt".into(), Arc::new(vec![review]));
        });
        harness.rebuild();
        harness.paint();
        let column = find(harness.root_id(), "conversation column").unwrap();
        let at = |name: &str| {
            let id = find(column, name).unwrap_or_else(|| panic!("no {name} on the page"));
            let rect = id.get_visual_rect_no_clip();
            assert!(rect.height() > 0.0, "{name} is not shown");
            rect
        };
        // The run and the review are the bar's, and so is Merge: none of
        // their views, by the names they have there, is on the page.
        for theirs in [
            "agents-dropdown",
            "agent-review",
            "agent-run",
            "merge-split",
            "merge-popover",
            "merge-action",
        ] {
            let found = all_named(column, theirs);
            assert!(
                found.is_empty(),
                "the page has {theirs}: {:?}",
                found.iter().map(|id| id.debug_name()).collect::<Vec<_>>()
            );
        }
        let (line, description) = (at("summary line"), at("card:description"));
        assert!(
            description.contains(line.origin()) && line.y1 <= description.y1,
            "the line at {line:?} is not in the description {description:?}"
        );
        let order = ["card:description", "composer", "timeline"];
        for pair in order.windows(2) {
            let (above, below) = (at(pair[0]), at(pair[1]));
            assert!(
                above.y1 <= below.y0,
                "{} at {above:?} is not over {} at {below:?}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(
            cards_in_order(&harness),
            [
                "card:description",
                "card:event",
                "card:thread",
                "card:review",
                "card:comment",
                "card:event"
            ]
        );
        let events = all_named(column, "card:event");
        assert!(
            events[0].get_visual_rect_no_clip().y0 < events[1].get_visual_rect_no_clip().y0,
            "the merge is not over the opening"
        );

        let mut all = Vec::new();
        names(column, &mut all);
        let text = all.join("\n");
        assert!(
            !text.contains("\"Fence the manifest\""),
            "the page repeats the title"
        );
        assert!(!shown(&harness, "older cards"), "nothing is folded away");
    }

    /// Close as issue's form, which a menu unfolds, sits over the
    /// description, with no merge box on the page, since merging is the
    /// bar's, and the timeline runs straight on from the composer, with
    /// no strip of finding filters between them.
    #[test]
    fn the_forms_sit_in_their_places() {
        let (mut harness, app, state) = app_harness();
        let at: jiff::Timestamp = "2026-09-18T09:59:00Z".parse().unwrap();
        app.update_review_finding(
            "wt".into(),
            crate::review::model::ReviewFindingSummary {
                finding: perch_github::Finding {
                    id: "f-1".into(),
                    item_id: "wt".into(),
                    stable_key: "lock".into(),
                    thread_id: "finding-1".into(),
                    created_group_id: None,
                    disposition: perch_github::FindingDisposition::Open,
                    verification: perch_github::FindingVerification::Unverified,
                    dismissal_reason: None,
                    possible_duplicate_of: None,
                    duplicate_of: None,
                    last_verified_snapshot_id: None,
                    last_verified_head_sha: None,
                    last_checked_snapshot_id: None,
                    last_checked_head_sha: None,
                    created_at: at,
                    updated_at: at,
                },
                severity: perch_github::FindingSeverity::High,
                attributions: Vec::new(),
                seen_in_reviews: Vec::new(),
                backend: None,
                model: None,
                original_review_number: None,
                title: "The lock is held across an await.".into(),
            },
        );
        crate::review::close_as_issue::unfold(app, state, "wt");
        harness.set_size(1400.0, 3000.0);
        settle(&mut harness);
        let column = find(harness.root_id(), "conversation column").unwrap();
        // The column's own rows first, as the forms have boxes of their
        // own called composer.
        let at = |name: &str| {
            let id = column
                .children()
                .into_iter()
                .find(|row| row.debug_name().contains(name))
                .or_else(|| find(column, name))
                .unwrap_or_else(|| panic!("no {name} on the page"));
            let rect = id.get_visual_rect_no_clip();
            assert!(rect.height() > 0.0, "{name} is not shown");
            rect
        };
        assert!(
            find(column, "merge-action").is_none(),
            "a merge box on the page"
        );
        let order = ["close-as-issue", "card:description", "composer", "timeline"];
        assert!(
            find(column, "review-finding-filters").is_none(),
            "the findings strip is back"
        );
        for pair in order.windows(2) {
            let (above, below) = (at(pair[0]), at(pair[1]));
            assert!(
                above.y1 <= below.y0,
                "{} at {above:?} is not over {} at {below:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// A dismissed finding's thread is left out of the timeline, with one
    /// row under it that shows it and then hides it again; with nothing
    /// resolved, there is no row.
    #[test]
    fn resolved_findings_fold_into_one_row() {
        let (mut harness, app, state) = app_harness();
        settle(&mut harness);
        assert!(
            !shown(&harness, "resolved findings"),
            "a row with nothing resolved"
        );
        assert!(shown(&harness, "card:thread"));

        let at: jiff::Timestamp = "2026-09-18T09:59:00Z".parse().unwrap();
        app.update_review_finding(
            "wt".into(),
            crate::review::model::ReviewFindingSummary {
                finding: perch_github::Finding {
                    id: "f-1".into(),
                    item_id: "wt".into(),
                    stable_key: "fence".into(),
                    thread_id: "t1".into(),
                    created_group_id: None,
                    disposition: perch_github::FindingDisposition::Dismissed,
                    verification: perch_github::FindingVerification::Unverified,
                    dismissal_reason: None,
                    possible_duplicate_of: None,
                    duplicate_of: None,
                    last_verified_snapshot_id: None,
                    last_verified_head_sha: None,
                    last_checked_snapshot_id: None,
                    last_checked_head_sha: None,
                    created_at: at,
                    updated_at: at,
                },
                severity: perch_github::FindingSeverity::Low,
                attributions: Vec::new(),
                seen_in_reviews: Vec::new(),
                backend: None,
                model: None,
                original_review_number: None,
                title: "The fence is not needed.".into(),
            },
        );
        // As the window does when a finding changes.
        state.apply_review_finding_filter(
            state.review_finding_filter.get_untracked(),
            &app.review_findings_for("wt"),
        );
        settle(&mut harness);
        assert!(
            !shown(&harness, "card:thread"),
            "the dismissed thread is shown"
        );
        assert!(shown(&harness, "\"Show 1 resolved finding\""));

        let row = find(harness.root_id(), "show-resolved").unwrap();
        to_top(&mut harness, row);
        click_on(&mut harness, row);
        settle(&mut harness);
        assert!(
            shown(&harness, "card:thread"),
            "the thread did not come back"
        );
        assert!(shown(&harness, "\"Hide 1 resolved finding\""));

        let row = find(harness.root_id(), "show-resolved").unwrap();
        to_top(&mut harness, row);
        click_on(&mut harness, row);
        settle(&mut harness);
        assert!(!shown(&harness, "card:thread"), "the thread stayed");
    }

    /// Shown, a resolved finding's thread older than the cards the
    /// timeline keeps unfolded comes back unfolded, rather than among the
    /// folded cards where the click would seem to do nothing.
    #[test]
    fn an_old_resolved_finding_comes_back_unfolded() {
        let (mut harness, app, state) =
            app_harness_on(crate::backend::Backend::detached(), long_discussion());
        let at: jiff::Timestamp = "2026-09-18T09:59:00Z".parse().unwrap();
        app.update_review_finding(
            "wt".into(),
            crate::review::model::ReviewFindingSummary {
                finding: perch_github::Finding {
                    id: "f-1".into(),
                    item_id: "wt".into(),
                    stable_key: "fence".into(),
                    thread_id: "t1".into(),
                    created_group_id: None,
                    disposition: perch_github::FindingDisposition::Dismissed,
                    verification: perch_github::FindingVerification::Unverified,
                    dismissal_reason: None,
                    possible_duplicate_of: None,
                    duplicate_of: None,
                    last_verified_snapshot_id: None,
                    last_verified_head_sha: None,
                    last_checked_snapshot_id: None,
                    last_checked_head_sha: None,
                    created_at: at,
                    updated_at: at,
                },
                severity: perch_github::FindingSeverity::Low,
                attributions: Vec::new(),
                seen_in_reviews: Vec::new(),
                backend: None,
                model: None,
                original_review_number: None,
                title: "The fence is not needed.".into(),
            },
        );
        state.apply_review_finding_filter(
            state.review_finding_filter.get_untracked(),
            &app.review_findings_for("wt"),
        );
        settle(&mut harness);
        assert!(shown(&harness, "older cards"), "nothing is folded");

        let row = find(harness.root_id(), "show-resolved").unwrap();
        to_top(&mut harness, row);
        click_on(&mut harness, row);
        settle(&mut harness);
        assert!(shown(&harness, "card:thread"), "the thread is folded away");
    }

    /// A discussion with seven more comments, between the review and the
    /// merge, so the timeline has twelve cards: the newest eight are the
    /// merge and these.
    fn long_discussion() -> Discussion {
        let mut discussion = discussion();
        for i in 0..7 {
            discussion.comments.push(IssueComment {
                node_id: String::new(),
                reactions: Default::default(),
                id: 100 + i,
                user: "rohan".into(),
                user_avatar_url: String::new(),
                body: format!("Comment {i}."),
                created_at: at(&format!("2026-09-17T0{i}:00:00Z")),
                updated_at: None,
                html_url: format!("https://github.com/criccomini/perch/pull/57#issuecomment-{i}"),
                viewer_can_delete: false,
                viewer_can_update: false,
            });
        }
        discussion
    }

    /// The timeline cards on the page, the description left out.
    fn timeline_cards(harness: &HeadlessHarness) -> usize {
        cards_in_order(harness)
            .iter()
            .filter(|card| *card != "card:description")
            .count()
    }

    /// Twelve cards are the newest eight and Show 4 older, which shows the
    /// rest. They stay shown when Conversation comes back from Code, and
    /// fold again when another item opens.
    #[test]
    fn the_timeline_folds_its_older_cards_until_asked() {
        let (mut harness, _, state) =
            app_harness_on(crate::backend::Backend::detached(), long_discussion());
        assert_eq!(timeline_cards(&harness), super::NEWEST_SHOWN);
        assert!(shown(&harness, "older cards"));
        assert!(find(harness.root_id(), "Show 4 older").is_some());
        let cards = cards_in_order(&harness);
        assert_eq!(cards[1], "card:event", "the merge is not first");
        assert!(cards[2..].iter().all(|card| card == "card:comment"));

        show_older(&mut harness);
        assert_eq!(timeline_cards(&harness), 12);
        assert!(!shown(&harness, "older cards"));
        assert_eq!(
            cards_in_order(&harness).last().map(String::as_str),
            Some("card:event"),
            "the opening is not last"
        );

        state.show_tab(crate::review::model::ItemTab::Code);
        harness.rebuild();
        harness.paint();
        state.show_tab(crate::review::model::ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        assert_eq!(timeline_cards(&harness), 12, "Code folded them again");

        state.item.update(|item| {
            if let Some(item) = item {
                Arc::make_mut(item).id = "wt2".into();
            }
        });
        harness.rebuild();
        harness.paint();
        assert_eq!(
            timeline_cards(&harness),
            super::NEWEST_SHOWN,
            "another item kept them shown"
        );
        assert!(shown(&harness, "older cards"));
    }

    /// A card asked for that is folded away, as a question's run asks for
    /// its thread, shows the older cards and comes into view; and Cmd-F's
    /// bar shows them all, so it looks through every card.
    #[test]
    fn a_card_asked_for_or_find_shows_the_older_cards() {
        let (mut harness, _, state) =
            app_harness_on(crate::backend::Backend::detached(), long_discussion());
        harness.set_size(1400.0, 500.0);
        harness.rebuild();
        harness.paint();
        assert_eq!(timeline_cards(&harness), super::NEWEST_SHOWN);

        state
            .focus_card
            .set(Some(crate::review::doc::CardId::Entry("comment:1".into())));
        settle(&mut harness);
        assert_eq!(timeline_cards(&harness), 12);
        assert!(
            card_shown(&harness, "card:comment", false),
            "the oldest comment is not in view"
        );

        let (mut harness, _, state) =
            app_harness_on(crate::backend::Backend::detached(), long_discussion());
        assert_eq!(timeline_cards(&harness), super::NEWEST_SHOWN);
        state.conversation_find_open.set(true);
        settle(&mut harness);
        assert_eq!(timeline_cards(&harness), 12, "find sees only the newest");
    }

    /// A notification's open, or a question's run whose thread the filter
    /// hid, asks for its card to be focused once it is laid out: a card
    /// folded away shows the older cards and comes into view, and so does
    /// one asked for before the cards it lands among have come.
    #[test]
    fn a_card_asked_for_ahead_of_its_layout_shows_the_older_cards() {
        let comment = || Some(crate::review::doc::CardId::Entry("comment:1".into()));
        let (mut harness, _, state) =
            app_harness_on(crate::backend::Backend::detached(), long_discussion());
        harness.set_size(1400.0, 500.0);
        settle(&mut harness);
        assert_eq!(timeline_cards(&harness), super::NEWEST_SHOWN);
        state.pending_focus_card.set(comment());
        settle(&mut harness);
        assert_eq!(timeline_cards(&harness), 12);
        assert_eq!(state.focus_card.get_untracked(), comment());
        assert!(
            card_shown(&harness, "card:comment", false),
            "the oldest comment is not in view"
        );

        // Asked for before the discussion with it has landed.
        let mut early = discussion();
        early.comments.clear();
        let (mut harness, app, state) = app_harness_on(crate::backend::Backend::detached(), early);
        harness.set_size(1400.0, 500.0);
        settle(&mut harness);
        state.pending_focus_card.set(comment());
        settle(&mut harness);
        assert_eq!(state.pending_focus_card.get_untracked(), comment());
        deliver(app, state, long_discussion());
        settle(&mut harness);
        assert_eq!(
            timeline_cards(&harness),
            12,
            "the older cards stayed folded"
        );
        assert_eq!(state.focus_card.get_untracked(), comment());
        assert!(
            card_shown(&harness, "card:comment", false),
            "the oldest comment is not in view"
        );
    }

    /// A card with a composer open in it stays on the page when newer
    /// cards push it past the newest: the older cards unfold rather than
    /// take it away, and its box and the keyboard with it. They stay
    /// unfolded once the composer closes.
    #[test]
    fn a_card_being_edited_is_not_folded_away_by_newer_ones() {
        use crate::review::model::Destination;
        let mut discussion = long_discussion();
        // The oldest of the seven, the eighth newest card.
        let edited = discussion
            .comments
            .iter_mut()
            .find(|c| c.id == 100)
            .unwrap();
        edited.node_id = "IC_100".into();
        edited.viewer_can_update = true;
        let (mut harness, app, state) =
            app_harness_on(crate::backend::Backend::detached(), discussion.clone());
        assert_eq!(timeline_cards(&harness), super::NEWEST_SHOWN);
        state.open_composer_with(
            None,
            None,
            "Comment 0, edited.".into(),
            Destination::GitHub,
            Some("IC_100".into()),
        );
        settle(&mut harness);
        let editor = find(harness.root_id(), "comment-editor").unwrap();
        let typing = find(editor, "composer-box").expect("the card holds the editor");
        let editing = state.composer_box.get_untracked();
        assert!(
            editing.is_some(),
            "the editor's box is not where `a` finds it"
        );

        discussion.comments.push(IssueComment {
            id: 2,
            node_id: String::new(),
            user: "rohan".into(),
            user_avatar_url: String::new(),
            body: "One more thing.".into(),
            created_at: at("2026-09-18T09:00:00Z"),
            updated_at: None,
            html_url: "https://github.com/criccomini/perch/pull/57#issuecomment-2".into(),
            reactions: Default::default(),
            viewer_can_delete: false,
            viewer_can_update: false,
        });
        deliver(app, state, discussion);
        settle(&mut harness);
        assert_eq!(timeline_cards(&harness), 13, "the edited card folded away");
        let editor = find(harness.root_id(), "comment-editor").unwrap();
        assert_eq!(
            find(editor, "composer-box"),
            Some(typing),
            "the editor was built again"
        );
        assert_eq!(state.composer_box.get_untracked(), editing);

        state.close_composer();
        settle(&mut harness);
        assert_eq!(timeline_cards(&harness), 13, "closing it folded them");
    }

    /// A long description is cut to a few hundred pixels, with Show more
    /// under it, which draws all of it, and Show less, which cuts it
    /// again; Cmd-F's bar draws it whole. A short one has neither.
    #[test]
    fn a_long_description_is_cut_until_show_more() {
        let (harness, _, _) = app_harness();
        assert!(!shown(&harness, "description-more"), "a short one is cut");

        let mut long = discussion();
        long.body = "A paragraph of the description.\n\n".repeat(40);
        let (mut harness, _, state) = app_harness_on(crate::backend::Backend::detached(), long);
        settle(&mut harness);
        let text = |harness: &HeadlessHarness| {
            let id = find(harness.root_id(), "description-text").unwrap();
            id.get_visual_rect_no_clip().height()
        };
        assert_eq!(text(&harness), crate::review::cards::DESCRIPTION_CLAMP);
        assert!(find(harness.root_id(), "Show more").is_some());
        // The cut's last lines fade out, flush with its foot.
        let fade = find(harness.root_id(), "description-fade").unwrap();
        let cut = find(harness.root_id(), "description-text")
            .unwrap()
            .get_visual_rect_no_clip();
        let faded = fade.get_visual_rect_no_clip();
        assert_eq!((faded.y1, faded.width()), (cut.y1, cut.width()));
        assert!(faded.height() > 20.0, "{faded:?}");
        let more = find(harness.root_id(), "description-more").unwrap();
        click_on(&mut harness, more);
        assert!(!shown(&harness, "description-fade"), "the whole text fades");
        assert!(text(&harness) > 2.0 * crate::review::cards::DESCRIPTION_CLAMP);
        assert!(find(harness.root_id(), "Show less").is_some());
        // Under the whole of it now, a screen or more down.
        more.scroll_to(None);
        settle(&mut harness);
        click_on(&mut harness, more);
        assert_eq!(text(&harness), crate::review::cards::DESCRIPTION_CLAMP);
        // And the cut text is back in view, not the timeline far below.
        settle(&mut harness);
        let viewport = find(harness.root_id(), "conversation")
            .unwrap()
            .get_visual_rect_no_clip();
        let cut = find(harness.root_id(), "description-text")
            .unwrap()
            .get_visual_rect_no_clip();
        assert!(
            cut.y0 >= viewport.y0 && cut.y0 < viewport.y1,
            "the description at {cut:?} is out of {viewport:?}"
        );
        let more = more.get_visual_rect_no_clip();
        assert!(
            more.y0 >= viewport.y0 && more.y1 <= viewport.y1,
            "Show more at {more:?} is out of {viewport:?}"
        );

        state.conversation_find_open.set(true);
        settle(&mut harness);
        assert!(text(&harness) > 2.0 * crate::review::cards::DESCRIPTION_CLAMP);
        assert!(!shown(&harness, "description-more"));
    }

    /// The page's end can be scrolled to, whatever wraps on it. Under a
    /// width cap taffy served the column's unwrapped height for the pass
    /// at the cap's width, and the oldest cards under long comments sat
    /// past the scroll's end.
    #[test]
    fn the_end_of_the_page_can_be_scrolled_into_view_when_its_text_and_rows_wrap() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let mut pr = pull();
        pr.labels = (0..40)
            .map(|i| perch_github::Label {
                name: format!("label-{i}"),
                color: "0e8a16".into(),
            })
            .collect();
        let now = at("2026-09-18T09:59:00Z");
        let mut item = Item::new("criccomini/perch", now);
        item.id = "wt".into();
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(crate::review::model::Phase::Ready);
        state.pull_request.set(Some(Arc::new(pr)));
        let mut discussion = discussion();
        for i in 0..3 {
            discussion.comments.push(IssueComment {
                node_id: String::new(),
                reactions: Default::default(),
                id: 100 + i,
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "A sentence that runs on well past the column's width, ".repeat(12),
                created_at: at("2026-09-15T11:00:00Z"),
                updated_at: None,
                html_url: format!("https://github.com/criccomini/perch/pull/57#issuecomment-{i}"),
                viewer_can_delete: false,
                viewer_can_update: false,
            });
        }
        state.discussion.set(Some(Arc::new(discussion)));
        state.composer_autofocus.set(false);
        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();

        // The column is at its full width, and its comments wrap.
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let column = find(harness.root_id(), "conversation column").unwrap();
        assert_eq!(
            harness.get_layout_rect(column).width(),
            crate::theme::CONVERSATION_WIDTH
        );

        // What the scroll thinks it holds is what the column is, with the
        // page's padding around it.
        let child = scroll.children()[0];
        assert_eq!(
            harness.get_layout_rect(child).height(),
            harness.get_layout_rect(column).height() + 2.0 * crate::theme::CONVERSATION_PADDING,
            "the scroll's content is not the column"
        );

        // And so the oldest card, at the column's end, can be reached.
        let viewport = scroll.get_visual_rect_no_clip();
        harness.scroll_down(viewport.center().x, viewport.center().y, 100_000.0);
        harness.rebuild();
        harness.paint();
        let end = column.get_visual_rect_no_clip();
        assert_eq!(
            end.y1,
            viewport.y1 - crate::theme::CONVERSATION_PADDING,
            "the column's end at {end:?} is not at the end of the scroll {viewport:?}"
        );
        let oldest = *card_views(&harness).last().unwrap();
        assert!(oldest.get_visual_rect_no_clip().y1 <= viewport.y1);
    }

    #[test]
    fn the_cards_are_newest_first_with_the_thread_between_the_merge_and_the_review() {
        let (harness, _, state) = app_harness();
        assert_eq!(state.tab.get(), crate::review::model::ItemTab::Conversation);
        assert_eq!(
            cards_in_order(&harness),
            [
                "card:description",
                "card:event",
                "card:thread",
                "card:review",
                "card:comment",
                "card:event"
            ]
        );
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let text = all.join("\n");
        for expected in [
            "Thanks",
            "approved",
            "Perch only",
            "You",
            "merged",
            "Label: \"src/manifest.rs\"",
            "Ask about this pull request",
        ] {
            assert!(text.contains(expected), "no {expected:?} on screen");
        }
        assert!(!shown(&harness, "Nothing here yet"));
    }

    /// What GitHub's timeline says was done is a line each, with the
    /// pieces the line is made of: a label in its own pill, a commit by
    /// its sha and subject, a review request by who it is of, and a
    /// reference by where it points. A commit is signed by the name git
    /// recorded, since it has no account behind it.
    ///
    /// Nothing on a line spills past it: the time at the end stays on
    /// screen however long the commit's subject is.
    #[test]
    fn what_github_says_was_done_is_a_line_each_with_its_own_pieces() {
        let mut discussion = discussion();
        let event = |kind, when: &str| PullEvent::new(kind, at(when), "rohan");
        discussion.events.extend([
            event(
                PullEventKind::Labeled {
                    label: perch_github::Label {
                        name: "needs work".into(),
                        color: "d73a4a".into(),
                    },
                },
                "2026-09-15T08:00:00Z",
            ),
            PullEvent::new(
                PullEventKind::Committed {
                    sha: "abc1234def5678".into(),
                    author: "Chris Riccomini".into(),
                    login: String::new(),
                    subject: "Fence the manifest ".repeat(12),
                },
                at("2026-09-15T09:00:00Z"),
                "",
            ),
            event(
                PullEventKind::ReviewRequested {
                    who: "chris".into(),
                },
                "2026-09-15T10:00:00Z",
            ),
            event(
                PullEventKind::CrossReferenced {
                    reference: perch_github::Reference {
                        repo_full_name: "slatedb/slatedb".into(),
                        number: 2098,
                        title: "Use remote_seq".into(),
                        pull: true,
                        html_url: "https://github.com/slatedb/slatedb/pull/2098".into(),
                    },
                },
                "2026-09-15T10:30:00Z",
            ),
        ]);
        let (mut harness, _, _) = app_harness_on(crate::backend::Backend::detached(), discussion);
        // Nine cards: the opening is folded away.
        show_older(&mut harness);

        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let text = all.join("\n");
        for expected in [
            "needs work",
            "abc1234",
            "Chris Riccomini",
            "requested a review from chris",
            "slatedb/slatedb#2098",
            "Use remote_seq",
        ] {
            assert!(text.contains(expected), "no {expected:?} on screen");
        }
        assert!(!text.contains("abc1234def5678"), "a sha is shown short");

        // The reference opens what it points at.
        let opened = crate::ui::widgets::opened;
        opened();
        let slug = find(harness.root_id(), "slatedb/slatedb#2098").unwrap();
        to_top(&mut harness, slug);
        let c = slug.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        assert_eq!(
            opened(),
            vec!["https://github.com/slatedb/slatedb/pull/2098".to_string()]
        );

        // Every line's pieces are inside the line: a subject too long
        // for the column is cut, and the time at the end stays on it.
        // The children's rects are the row's own coordinates.
        fn rows(id: floem::ViewId, out: &mut Vec<floem::ViewId>) {
            if id.debug_name().contains("card:event") {
                out.push(id);
            }
            for child in id.children() {
                rows(child, out);
            }
        }
        let mut found = Vec::new();
        rows(harness.root_id(), &mut found);
        assert_eq!(found.len(), 6, "opened, four of these, and merged");
        for row in found {
            let width = harness.get_layout_rect(row).width();
            let last = *row.children().last().unwrap();
            for child in row.children() {
                let rect = harness.get_layout_rect(child);
                assert!(
                    rect.x1 <= width + 0.5,
                    "{} runs past its line: {rect:?} in a row {width} wide",
                    child.debug_name()
                );
            }
            assert!(
                harness.get_layout_rect(last).width() > 0.0,
                "the time is off the end of {}",
                row.debug_name()
            );
        }
    }

    /// A commit's line links what it names: its author's page when GitHub
    /// tied the commit to an account, the commit itself, and a file its
    /// subject names. A commit the checkout has opens on Files by itself;
    /// one it lacks opens under the pull request on GitHub. A file opens
    /// on Files.
    #[test]
    fn a_commit_line_links_its_author_its_sha_and_the_file_it_names() {
        let (here, away) = ("a".repeat(40), "b".repeat(40));
        let mut discussion = discussion();
        let commit = |sha: &str, login: &str, subject: &str, when: &str| {
            PullEvent::new(
                PullEventKind::Committed {
                    sha: sha.into(),
                    author: "Chris".into(),
                    login: login.into(),
                    subject: subject.into(),
                },
                at(when),
                "",
            )
        };
        discussion.events.extend([
            commit(
                &here,
                "criccomini",
                "Fence the manifest",
                "2026-09-15T09:00:00Z",
            ),
            commit(&away, "", "Update src/manifest.rs", "2026-09-15T10:00:00Z"),
        ]);
        let (backend, mut commands) = crate::backend::Backend::recording();
        let checked_out = here.clone();
        let (mut harness, _, state) = app_harness_prepared(backend, discussion, move |state| {
            state.commits.set(Arc::new(vec![perch_git::Commit {
                sha: checked_out,
                subject: "Fence the manifest".into(),
                body: String::new(),
                author: "Chris".into(),
                authored_at: at("2026-09-15T09:00:00Z"),
            }]));
            state.tree.set(Arc::new(vec![perch_git::TreeEntry::new(
                "src/manifest.rs",
                false,
            )]));
        });
        let opened = crate::ui::widgets::opened;
        opened();
        let click = |harness: &mut HeadlessHarness, needle: &str| {
            let line = on_event_line(harness.root_id(), needle).0;
            to_top(harness, line);
            let at = on_event_line(harness.root_id(), needle).1.center();
            harness.click(at.x, at.y);
        };

        // The account's login, not git's name, as every other line names
        // someone; a commit with no account is signed by git's name, which
        // leads nowhere.
        click(&mut harness, "criccomini");
        assert_eq!(opened(), ["https://github.com/criccomini"]);
        click(&mut harness, "Chris");
        assert!(opened().is_empty(), "git's name led somewhere");

        // The line is one text: a drag from the subject back over the sha
        // takes both, to copy, and opens nothing.
        let line = on_event_line(harness.root_id(), "Fence").0;
        to_top(&mut harness, line);
        let (line, subject) = on_event_line(harness.root_id(), "Fence");
        let (other, sha) = on_event_line(harness.root_id(), "aaaaaaa");
        assert_eq!(line, other, "the sha and the subject are one text");
        let y = subject.center().y;
        harness.pointer_down(subject.x1 - 1.0, y);
        harness.pointer_move(subject.x0, y);
        harness.pointer_move(sha.x0 + 1.0, y);
        harness.pointer_up(sha.x0 + 1.0, y);
        assert!(opened().is_empty(), "a drag is not a click");
        assert_eq!(copy(&mut harness), ["aaaaaaa Fence"]);

        click(&mut harness, "bbbbbbb");
        assert_eq!(
            opened(),
            vec![format!(
                "https://github.com/criccomini/perch/pull/57/commits/{away}"
            )]
        );

        while commands.try_recv().is_ok() {}
        click(&mut harness, "aaaaaaa");
        assert!(opened().is_empty(), "the checkout has it");
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::SetScope { item_id, scope: perch_git::Scope::Commit { sha }, .. })
                if item_id == "wt" && sha == here
        ));
        assert_eq!(state.tab.get_untracked(), crate::review::ItemTab::Code);

        state.show_tab(crate::review::ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        click(&mut harness, "src/manifest.rs");
        assert_eq!(state.tab.get_untracked(), crate::review::ItemTab::Code);
        harness.rebuild();
        assert_eq!(
            state.active.get_untracked().as_deref(),
            Some("src/manifest.rs"),
            "Files took it"
        );
        assert!(opened().is_empty());
    }

    /// A name on a card is a link to its page on GitHub: the author of a
    /// comment, and who opened the pull request on its event line. The
    /// reader's own thread says "You" until GitHub has said who they are,
    /// and "You" is no link, in look or in deed.
    #[test]
    fn a_name_on_a_card_is_a_link_to_its_page_on_github() {
        use crate::theme;
        use floem::peniko::Color;
        use floem::style::{BorderBottomColor, Cursor, CursorStyle};
        let (mut harness, app, _) = app_harness();
        let opened = crate::ui::widgets::opened;
        opened();
        // The name label under a card, and the click that reaches it. The
        // lookup is scoped to the card: chris also reviewed, so the strip
        // under the header names them first, and rohan wrote the
        // description as well as opening.
        let name_in = |card: floem::ViewId, name: &str| {
            find(card, &format!("name label - Label: {name:?}"))
                .unwrap_or_else(|| panic!("no name {name:?} on {}", card.debug_name()))
        };
        let click = |harness: &mut HeadlessHarness, id: floem::ViewId| {
            let c = id.get_visual_rect_no_clip().center();
            assert_eq!(
                harness.element_id_at(c.x, c.y).map(|e| e.owning_id()),
                Some(id),
                "{} is not under the pointer at {c:?}",
                id.debug_name()
            );
            harness.click(c.x, c.y);
        };
        let root = harness.root_id();
        let comment = find(root, "card:comment").unwrap();
        to_top(&mut harness, comment);
        click(&mut harness, name_in(comment, "chris"));
        assert_eq!(opened(), vec!["https://github.com/chris".to_string()]);
        to_top(&mut harness, on_event_line(root, "rohan").0);
        let rohan = on_event_line(root, "rohan").1.center();
        harness.click(rohan.x, rohan.y);
        assert_eq!(opened(), vec!["https://github.com/rohan".to_string()]);

        // The reader: "You" has no underline and no pointer, and leads
        // nowhere; once GitHub has named them, the same label is a link.
        let thread = find(root, "card:thread").unwrap();
        let you = name_in(thread, "You");
        // The summary over the cards puts the thread under the fold.
        you.scroll_to(None);
        harness.rebuild();
        harness.paint();
        let c = you.get_visual_rect_no_clip().center();
        harness.pointer_move(c.x, c.y);
        harness.recompute_styles();
        let plain = harness.get_computed_style(you);
        assert_eq!(
            plain.get(BorderBottomColor),
            Some(Color::TRANSPARENT.into())
        );
        assert_ne!(plain.get(Cursor), Some(CursorStyle::Pointer));
        click(&mut harness, you);
        assert_eq!(opened(), Vec::<String>::new(), "You led somewhere");
        app.viewer.set(Some(("me".into(), String::new())));
        harness.rebuild();
        harness.paint();
        let me = name_in(thread, "me");
        assert_eq!(me, you, "the label was rebuilt rather than reworded");
        harness.pointer_move(c.x, c.y);
        harness.recompute_styles();
        let linked = harness.get_computed_style(me);
        assert_eq!(
            linked.get(BorderBottomColor),
            Some(theme::fg_muted().into())
        );
        assert_eq!(linked.get(Cursor), Some(CursorStyle::Pointer));
        click(&mut harness, me);
        assert_eq!(opened(), vec!["https://github.com/me".to_string()]);
    }

    /// The Opened line names the pull request's number after "opened",
    /// and the number opens it on github.com.
    #[test]
    fn the_opened_line_links_the_pull_requests_number() {
        let (mut harness, ..) = app_harness();
        let opened = crate::ui::widgets::opened;
        opened();
        // The opening is the oldest card, at the page's end.
        let line = on_event_line(harness.root_id(), "#57").0;
        to_top(&mut harness, line);
        let (line, number) = on_event_line(harness.root_id(), "#57");
        assert_eq!(
            crate::review::linked_text::with_text(line, str::to_string).as_deref(),
            Some("rohan opened #57")
        );
        let c = number.center();
        harness.click(c.x, c.y);
        assert_eq!(opened(), ["https://github.com/criccomini/perch/pull/57"]);
    }

    /// Code a comment names is a link to it in Files. A path and a line in
    /// a code span leaves Conversation for Files at a click and lands on
    /// the line once the file is in; a name in a code span is asked about,
    /// and the path is not.
    #[test]
    fn a_path_a_comment_names_opens_on_files_at_its_line() {
        use crate::backend::Command;
        use crate::review::model::ItemTab;
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut discussion = discussion();
        discussion.comments[0].body = "See `manifest.rs:2` and `Manifest`.".into();
        let (mut harness, _, state) = app_harness_prepared(backend, discussion, |state| {
            state.tree.set(Arc::new(vec![
                perch_git::TreeEntry::new("src", true),
                perch_git::TreeEntry::new("src/manifest.rs", false),
            ]));
            state.set_server_status(
                perch_lsp::ServerKind::RustAnalyzer,
                perch_lsp::Status::Ready,
            );
        });
        let mut asked = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let Command::Names { names, .. } = command {
                asked.extend(names.into_iter().map(|n| n.key));
            }
        }
        assert_eq!(asked, vec!["Manifest".to_string()]);

        let comment = find(harness.root_id(), "card:comment").unwrap();
        to_top(&mut harness, comment);
        let text = find(comment, "LinkedText").unwrap();
        let links = crate::review::linked_text::links_on_first_line(&mut harness, text);
        assert_eq!(
            links.len(),
            1,
            "the path, and not the name no one has answered: {links:?}"
        );
        harness.click(links[0].x, links[0].y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(state.active.get().as_deref(), Some("src/manifest.rs"));
        // On its way already, for the thread's lines on Conversation.
        assert!(state.is_loading("src/manifest.rs"));

        let file = crate::backend::build_loaded_file(
            "src/manifest.rs".into(),
            "fn a() {}\nfn b() {}\nfn c() {}\n".into(),
            "fn a() {}\nfn old() {}\nfn c() {}\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        state.finish_load("src/manifest.rs", Arc::new(file));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.jump_to_line.get(), None, "the editor took the jump");
        assert_eq!(
            state.last_position.get(),
            Some((1, 0)),
            "the caret is on line 2"
        );
    }

    /// One of GitHub's review threads is a thread card at its first
    /// comment's time, opening with where it was left rather than "You
    /// asked": each comment carries its own byline, the reply under the
    /// comment it answers, and no Local chip. Ask about this on a comment
    /// opens the composer under the thread itself, with nothing quoted.
    #[test]
    fn a_github_thread_is_a_thread_card_whose_ask_opens_under_it() {
        let (mut harness, _, state) = app_harness();
        let discussion = discussion();
        let mut github = perch_github::comments::review_threads_of(&discussion, "wt", "h");
        assert_eq!(github.len(), 1);
        let github = github.remove(0);
        assert_eq!(github.id, "gh-31");
        state.set_comment_threads(Arc::new(vec![thread(), github]));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            cards_in_order(&harness),
            [
                "card:description",
                "card:event",
                "card:thread",
                "card:review",
                "card:thread",
                "card:comment",
                "card:event"
            ],
            "GitHub's thread at its comment's time, under the review it was left in"
        );
        let root = harness.root_id();
        let card = *all_named(root, "card:thread").last().unwrap();
        let mut all = Vec::new();
        names(card, &mut all);
        let text = all.join("\n");
        assert!(
            text.contains("thread-path - Label: \"src/manifest.rs\""),
            "the header names the file: {text}"
        );
        assert_eq!(
            text.matches("name label - Label: \"chris\"").count(),
            2,
            "a byline a comment"
        );
        assert!(
            !text.contains("Label: \"Local\""),
            "GitHub's thread is not marked Local"
        );
        assert!(!text.contains("Label: \"You\""), "nobody here asked");
        assert!(
            text.contains("[\"epoch\"]") && text.contains("[\"the reply\"]"),
            "{text}"
        );
        let bylines: Vec<_> = all
            .iter()
            .enumerate()
            .filter(|(_, n)| n.contains("name label - Label: \"chris\""))
            .map(|(i, _)| i)
            .collect();
        let reply_after_root = all[bylines[0]..bylines[1]]
            .iter()
            .any(|n| n.contains("[\"epoch\"]"));
        assert!(
            reply_after_root,
            "the reply's byline comes after the comment"
        );

        // The card is under the fold; bring it up and hover it for the Ask.
        card.scroll_to(None);
        harness.rebuild();
        harness.paint();
        let c = card.get_visual_rect_no_clip().center();
        harness.pointer_move(c.x, c.y);
        harness.rebuild();
        harness.paint();
        crate::review::cards::more::pick(&mut harness, card, "Ask About This");
        let composer = state.composer.get().expect("the composer opened");
        assert_eq!(composer.thread_id.as_deref(), Some("gh-31"));
        assert_eq!(
            composer.anchor.as_ref().map(|a| a.label()),
            Some("src/manifest.rs:41".to_string())
        );
        assert_eq!(
            state.composer_draft.get(),
            "",
            "the thread carries the comment; nothing is quoted"
        );
        assert!(
            find(card, "composer-box").is_some(),
            "the follow-up composer is inside the GitHub thread's card"
        );
    }

    /// Quote reply on a comment opens the composer over the timeline,
    /// bound for GitHub, with the comment quoted and room under
    /// it; Quote reply on the review then quotes it under what the reader
    /// typed, rather than throwing that away.
    #[test]
    fn quote_reply_quotes_a_card_into_the_composer_for_github() {
        let (mut harness, _, state) = app_harness();
        let quote = |harness: &mut HeadlessHarness, card: &str| {
            let card = find(harness.root_id(), card).unwrap();
            crate::review::cards::more::pick(harness, card, "Quote Reply");
        };

        quote(&mut harness, "card:comment");
        let composer = state.composer.get().expect("the composer opened");
        assert_eq!(
            (composer.thread_id, composer.anchor),
            (None, None),
            "over the timeline"
        );
        assert!(composer.destination.is_github());
        assert_eq!(state.composer_draft.get(), "> Thanks\n\n");
        let foot = find(harness.root_id(), "composer").unwrap();
        assert!(
            find(foot, "composer-box").is_some(),
            "the box is open over the timeline"
        );

        state
            .composer_draft
            .set("> Thanks\n\nYou're welcome.".into());
        quote(&mut harness, "card:review");
        assert_eq!(
            state.composer_draft.get(),
            "> Thanks\n\nYou're welcome.\n\n> Looks right.\n\n"
        );
    }

    /// Delete on a comment the reader may delete asks first, under the
    /// comment; Cancel leaves it. Delete then takes the comment out at
    /// once, puts a line in the timeline saying who deleted whose, and
    /// asks the backend. A card GitHub did not say the reader may delete
    /// offers no Delete.
    #[test]
    fn delete_asks_then_leaves_a_line_where_the_comment_was() {
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut discussion = discussion();
        discussion.comments[0].node_id = "IC_1".into();
        discussion.comments[0].user = "kvnloo".into();
        discussion.comments[0].viewer_can_delete = true;
        let (mut harness, app, state) = app_harness_on(backend, discussion);
        app.viewer.set(Some(("criccomini".into(), String::new())));
        let root = harness.root_id();
        let comment = find(root, "card:comment").unwrap();
        to_top(&mut harness, comment);
        crate::review::cards::more::pick(&mut harness, comment, "Delete\u{2026}");
        assert_eq!(state.confirm_comment.get().as_deref(), Some("IC_1"));
        assert!(shown(&harness, "delete-comment-confirmation"));
        let cancel = find(comment, "cancel-delete-comment").unwrap();
        click_on(&mut harness, cancel);
        assert_eq!(state.confirm_comment.get(), None);
        assert!(!shown(&harness, "delete-comment-confirmation"));

        while commands.try_recv().is_ok() {}
        let comment = find(harness.root_id(), "card:comment").unwrap();
        crate::review::cards::more::pick(&mut harness, comment, "Delete\u{2026}");
        let delete = find(comment, "confirm-delete-comment").unwrap();
        click_on(&mut harness, delete);
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::DeleteGithubComment { item_id, node_id, place: perch_github::CommentPlace::Conversation })
                if item_id == "wt" && node_id == "IC_1"
        ));
        let root = harness.root_id();
        assert!(find(root, "card:comment").is_none(), "the comment is gone");
        let mut all = Vec::new();
        names(root, &mut all);
        assert!(
            all.iter()
                .any(|n| n.contains("deleted a comment from kvnloo")),
            "the timeline says who deleted whose"
        );

        let review = find(root, "card:review").unwrap();
        let rows = crate::review::cards::more::rows(&mut harness, review);
        assert!(
            !rows.is_empty() && !rows.iter().any(|r| r.starts_with("Delete")),
            "a review is not deleted from here: {rows:?}"
        );
    }

    /// Edit on a comment the reader may edit opens the composer in the
    /// card, its words in it, the composer over the timeline keeping the
    /// box that asks; Update comment shows the new words at once and asks
    /// the backend. A card GitHub did not say the reader may edit offers
    /// no Edit.
    #[test]
    fn edit_opens_the_composer_in_the_card_and_updates_the_comment() {
        use crate::review::model::Destination;
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut discussion = discussion();
        discussion.comments[0].node_id = "IC_1".into();
        discussion.comments[0].viewer_can_update = true;
        let (mut harness, _, state) = app_harness_on(backend, discussion);
        let comment = find(harness.root_id(), "card:comment").unwrap();
        comment.scroll_to(None);
        harness.rebuild();
        harness.paint();
        crate::review::cards::more::pick(&mut harness, comment, "Edit");
        let composer = state.composer.get().expect("Edit opened the composer");
        assert_eq!(composer.editing.as_deref(), Some("IC_1"));
        assert_eq!(composer.thread_id, None);
        assert_eq!(composer.destination, Destination::GitHub);
        assert_eq!(state.composer_draft.get(), "Thanks");
        let comment = find(harness.root_id(), "card:comment").unwrap();
        let editor = find(comment, "comment-editor").expect("the card holds the editor");
        assert!(harness.get_layout_rect(editor).height() > 0.0);
        assert!(find(editor, "Label: \"Update comment\"").is_some());
        assert!(
            find(editor, "composer-destination").is_none(),
            "it goes nowhere else"
        );
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let boxes = all.iter().filter(|n| n.contains("composer-box")).count();
        assert_eq!(
            boxes, 1,
            "the card's alone; the page's composer keeps the box that asks"
        );

        // Opened again with other words, as the reader would type them.
        while commands.try_recv().is_ok() {}
        state.open_composer_with(
            None,
            None,
            "Thanks a lot".into(),
            Destination::GitHub,
            Some("IC_1".into()),
        );
        harness.rebuild();
        harness.paint();
        let comment = find(harness.root_id(), "card:comment").unwrap();
        let send = find(comment, "composer-send").expect("Update comment");
        click_on(&mut harness, send);
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::UpdateGithubComment { item_id, node_id, place: perch_github::CommentPlace::Conversation, body })
                if item_id == "wt" && node_id == "IC_1" && body == "Thanks a lot"
        ));
        assert!(state.composer.get().is_none(), "the edit closed");
        let kept = state.discussion.get().unwrap();
        assert_eq!(
            kept.comments[0].body, "Thanks a lot",
            "shown before GitHub answers"
        );
        let comment = find(harness.root_id(), "card:comment").unwrap();
        assert!(!shown(&harness, "comment-editor"));
        let mut all = Vec::new();
        names(comment, &mut all);
        assert!(
            all.iter().any(|n| n.contains("Thanks a lot")),
            "the card shows the new words"
        );

        let review = find(harness.root_id(), "card:review").unwrap();
        let rows = crate::review::cards::more::rows(&mut harness, review);
        assert!(
            !rows.is_empty() && !rows.contains(&"Edit".to_string()),
            "not without GitHub's say so: {rows:?}"
        );
    }

    /// The description and a review's summary, which GitHub says the
    /// reader may edit, are edited in their cards as a comment is, each
    /// by its own place; neither offers Edit without GitHub's say so.
    #[test]
    fn the_description_and_a_review_summary_are_edited_in_their_cards() {
        use crate::review::model::Destination;
        use perch_github::CommentPlace;
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut discussion = discussion();
        discussion.node_id = "PR_57".into();
        discussion.viewer_can_update = true;
        discussion.reviews[0].node_id = "PRR_9".into();
        discussion.reviews[0].viewer_can_update = true;
        let (mut harness, _, state) = app_harness_on(backend, discussion);

        let cases = [
            (
                "card:description",
                "PR_57",
                "The description.",
                CommentPlace::PullRequestBody,
            ),
            ("card:review", "PRR_9", "Looks right.", CommentPlace::Review),
        ];
        for (name, id, was, place) in cases {
            let card = find(harness.root_id(), name).unwrap();
            card.scroll_to(None);
            harness.rebuild();
            harness.paint();
            crate::review::cards::more::pick(&mut harness, card, "Edit");
            let composer = state
                .composer
                .get()
                .unwrap_or_else(|| panic!("{name}: no composer"));
            assert_eq!(composer.editing.as_deref(), Some(id), "{name}");
            assert_eq!(state.composer_draft.get(), was, "{name}");
            let card = find(harness.root_id(), name).unwrap();
            let editor = find(card, "comment-editor").expect("the card holds the editor");
            assert!(harness.get_layout_rect(editor).height() > 0.0, "{name}");
            assert!(
                find(editor, "Label: \"Update comment\"").is_some(),
                "{name}"
            );

            while commands.try_recv().is_ok() {}
            let words = format!("{was} Edited.");
            state.open_composer_with(
                None,
                None,
                words.clone(),
                Destination::GitHub,
                Some(id.into()),
            );
            harness.rebuild();
            harness.paint();
            let card = find(harness.root_id(), name).unwrap();
            click_on(
                &mut harness,
                find(card, "composer-send").expect("Update comment"),
            );
            match commands.try_recv() {
                Ok(crate::backend::Command::UpdateGithubComment {
                    item_id,
                    node_id,
                    place: asked,
                    body,
                }) => {
                    assert_eq!(
                        (item_id.as_str(), node_id.as_str(), asked),
                        ("wt", id, place),
                        "{name}"
                    );
                    assert_eq!(body, words, "{name}");
                }
                other => panic!("{name}: the edit was not asked for: {other:?}"),
            }
            assert!(state.composer.get().is_none(), "{name}: the edit closed");
            let card = find(harness.root_id(), name).unwrap();
            let mut all = Vec::new();
            names(card, &mut all);
            assert!(
                all.iter().any(|n| n.contains("Edited.")),
                "{name}: shows the new words"
            );
        }
        let kept = state.discussion.get().unwrap();
        assert_eq!(kept.body, "The description. Edited.");
        assert_eq!(kept.reviews[0].body, "Looks right. Edited.");

        // Without GitHub's say so, neither offers Edit.
        state.discussion.update(|d| {
            let d = Arc::make_mut(d.as_mut().unwrap());
            d.viewer_can_update = false;
            d.reviews[0].viewer_can_update = false;
        });
        harness.rebuild();
        harness.paint();
        for name in ["card:description", "card:review"] {
            let card = find(harness.root_id(), name).unwrap();
            card.scroll_to(None);
            harness.rebuild();
            harness.paint();
            let rows = crate::review::cards::more::rows(&mut harness, card);
            assert!(!rows.contains(&"Edit".to_string()), "{name}: {rows:?}");
        }
    }

    /// A description may be emptied. It and a comment keep the whitespace
    /// they open with, an indented code block's, losing only what trails.
    /// A review's summary emptied is not sent, as a comment's is not.
    #[test]
    fn an_edited_description_may_be_empty_and_keeps_its_leading_whitespace() {
        use crate::review::model::Destination;
        use perch_github::CommentPlace;
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut discussion = discussion();
        discussion.node_id = "PR_57".into();
        discussion.viewer_can_update = true;
        discussion.comments[0].node_id = "IC_1".into();
        discussion.comments[0].viewer_can_update = true;
        discussion.reviews[0].node_id = "PRR_9".into();
        discussion.reviews[0].viewer_can_update = true;
        let (mut harness, _, state) = app_harness_on(backend, discussion);
        let mut update = |harness: &mut HeadlessHarness, name: &str, id: &str, words: &str| {
            while commands.try_recv().is_ok() {}
            state.open_composer_with(
                None,
                None,
                words.into(),
                Destination::GitHub,
                Some(id.into()),
            );
            harness.rebuild();
            harness.paint();
            let card = find(harness.root_id(), name).unwrap();
            card.scroll_to(None);
            harness.rebuild();
            harness.paint();
            click_on(
                harness,
                find(card, "composer-send").expect("Update comment"),
            );
            match commands.try_recv() {
                Ok(crate::backend::Command::UpdateGithubComment {
                    node_id,
                    place,
                    body,
                    ..
                }) => {
                    assert_eq!(node_id, id);
                    Some((place, body))
                }
                _ => None,
            }
        };

        let cases = [
            ("", ""),
            (" \n\n", ""),
            (
                "    let x = 1;\n\nThe description.\n\n",
                "    let x = 1;\n\nThe description.",
            ),
        ];
        for (words, sent) in cases {
            let asked = update(&mut harness, "card:description", "PR_57", words);
            assert_eq!(
                asked,
                Some((CommentPlace::PullRequestBody, sent.to_string())),
                "{words:?}"
            );
            assert!(state.composer.get().is_none(), "{words:?}: the edit closed");
            assert_eq!(
                state.discussion.get().unwrap().body,
                sent,
                "{words:?}: shown at once"
            );
        }
        update(&mut harness, "card:description", "PR_57", "");
        let card = find(harness.root_id(), "card:description").unwrap();
        assert!(
            find(card, "No description.").is_some(),
            "an emptied description says so"
        );

        let words = "    let y = 2;\n\nThis panics.\n";
        let asked = update(&mut harness, "card:comment", "IC_1", words);
        let sent = "    let y = 2;\n\nThis panics.";
        assert_eq!(
            asked,
            Some((CommentPlace::Conversation, sent.to_string())),
            "a comment too"
        );
        assert_eq!(state.discussion.get().unwrap().comments[0].body, sent);

        let asked = update(&mut harness, "card:review", "PRR_9", "  ");
        assert_eq!(asked, None, "a summary needs words");
        assert!(state.composer.get().is_some(), "the edit stays open");
        assert_eq!(
            state.discussion.get().unwrap().reviews[0].body,
            "Looks right."
        );
    }

    /// An emptied edit of the description asks for a description, not
    /// about the pull request: Update comment sends it empty. A comment's
    /// and a review summary's ask for what goes in them.
    #[test]
    fn an_emptied_edit_says_what_goes_in_it() {
        use crate::review::model::Destination;
        let mut discussion = discussion();
        discussion.node_id = "PR_57".into();
        discussion.viewer_can_update = true;
        discussion.comments[0].node_id = "IC_1".into();
        discussion.comments[0].viewer_can_update = true;
        discussion.reviews[0].node_id = "PRR_9".into();
        discussion.reviews[0].viewer_can_update = true;
        let (mut harness, _, state) =
            app_harness_on(crate::backend::Backend::detached(), discussion);
        let cases = [
            ("card:description", "PR_57", "add-a-description…"),
            ("card:comment", "IC_1", "leave-a-comment…"),
            ("card:review", "PRR_9", "leave-a-review…"),
        ];
        for (name, id, placeholder) in cases {
            state.open_composer_with(
                None,
                None,
                String::new(),
                Destination::GitHub,
                Some(id.into()),
            );
            harness.rebuild();
            harness.paint();
            let card = find(harness.root_id(), name).unwrap();
            let editor = find(card, "comment-editor").expect("the card holds the editor");
            let mut all = Vec::new();
            names(editor, &mut all);
            let shown: Vec<_> = all
                .iter()
                .filter(|n| n.contains("composer-placeholder:"))
                .collect();
            let wanted = format!("composer-placeholder:{placeholder}");
            assert!(
                shown.len() == 1 && shown[0].contains(&wanted),
                "{name}: {shown:?}"
            );
        }
    }

    /// A comment GitHub did not say the reader may edit has no Edit.
    #[test]
    fn a_comment_the_reader_may_not_edit_offers_no_edit() {
        let mut discussion = discussion();
        discussion.comments[0].node_id = "IC_1".into();
        let (mut harness, _, _) = app_harness_on(crate::backend::Backend::detached(), discussion);
        let comment = find(harness.root_id(), "card:comment").unwrap();
        comment.scroll_to(None);
        harness.rebuild();
        harness.paint();
        let c = comment.get_visual_rect_no_clip().center();
        harness.pointer_move(c.x, c.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(
            crate::review::cards::more::rows(&mut harness, comment),
            [
                "Quote Reply",
                "Ask About This",
                "Copy Link",
                "Open on GitHub"
            ]
        );
    }

    /// Double-click the first word of the linked text under `card` that
    /// reads `text`, selecting it.
    fn select_first_word(harness: &mut HeadlessHarness, card: floem::ViewId, text: &str) {
        use crate::review::linked_text::{shown_under, with_text};
        card.scroll_to(None);
        harness.rebuild();
        harness.paint();
        let leaf = shown_under(card)
            .into_iter()
            .find(|id| with_text(*id, |t| t == text).unwrap_or(false))
            .unwrap_or_else(|| panic!("no text reading {text:?}"));
        let rect = leaf.get_visual_rect_no_clip();
        harness.double_click(rect.x0 + 3.0, rect.y0 + 8.0);
        harness.rebuild();
        harness.paint();
    }

    /// With words of a comment selected, Quote reply quotes those words
    /// and not the whole, as github.com does, and opening the `⋯` menu it
    /// is in leaves them selected; words selected in another card leave
    /// the comment's Quote reply quoting all of it.
    #[test]
    fn quote_reply_quotes_the_selection_when_there_is_one() {
        let mut discussion = discussion();
        discussion.comments[0].body = "Thanks for the fix".into();
        let (mut harness, _, state) =
            app_harness_on(crate::backend::Backend::detached(), discussion);
        let comment = find(harness.root_id(), "card:comment").unwrap();
        select_first_word(&mut harness, comment, "Thanks for the fix");
        // The menu takes the keyboard; the selection stays, and is still
        // there with the menu shut and nothing picked.
        let selected = || crate::review::linked_text::selected_under(comment);
        assert!(crate::review::cards::more::open(&mut harness, comment));
        assert_eq!(selected().as_deref(), Some("Thanks"));
        crate::ui::menu::close();
        harness.rebuild();
        harness.paint();
        assert_eq!(selected().as_deref(), Some("Thanks"));
        crate::review::cards::more::pick(&mut harness, comment, "Quote Reply");
        assert_eq!(state.composer_draft.get(), "> Thanks\n\n");

        state.close_composer();
        let description = find(harness.root_id(), "card:description").unwrap();
        select_first_word(&mut harness, description, "The description.");
        let comment = find(harness.root_id(), "card:comment").unwrap();
        crate::review::cards::more::pick(&mut harness, comment, "Quote Reply");
        assert_eq!(state.composer_draft.get(), "> Thanks for the fix\n\n");
    }

    /// An issue takes a comment from its Conversation as a pull request
    /// does, so its comments offer Quote reply too.
    #[test]
    fn quote_reply_on_an_issue_quotes_into_the_composer_for_github() {
        let (mut harness, _, state) =
            app_harness_prepared(crate::backend::Backend::detached(), discussion(), |state| {
                state.pull_request.set(None);
                state.issue.set(Some(Arc::new(perch_github::Issue {
                    viewer_can_update: false,
                    node_id: String::new(),
                    reactions: Default::default(),
                    number: 43,
                    title: "Fence the manifest".into(),
                    body: String::new(),
                    state: "open".into(),
                    state_reason: None,
                    author_login: "rohan".into(),
                    author_avatar_url: String::new(),
                    labels: vec![],
                    assignees: vec![],
                    comments: 1,
                    created_at: None,
                    updated_at: at("2026-09-16T11:40:00Z"),
                    repo_full_name: "criccomini/perch".into(),
                })));
            });
        let comment = find(harness.root_id(), "card:comment").unwrap();
        crate::review::cards::more::pick(&mut harness, comment, "Quote Reply");
        let composer = state.composer.get().expect("the composer opened");
        assert_eq!(
            (composer.thread_id, composer.anchor),
            (None, None),
            "over the timeline"
        );
        assert!(composer.destination.is_github());
        assert_eq!(state.composer_draft.get(), "> Thanks\n\n");
    }

    /// Quote reply on an agent's answer opens the thread's Follow up box
    /// with the selected words quoted, bound for the agent, never GitHub,
    /// and keeps what the reader had typed there above the quote.
    #[test]
    fn quote_reply_on_an_answer_follows_up_with_the_agent() {
        let (mut harness, _, state) = app_harness();
        let mut thread = thread();
        thread.messages[1].body = "because the lock".into();
        state.set_comment_threads(Arc::new(vec![thread]));
        harness.rebuild();
        harness.paint();
        let card = find(harness.root_id(), "card:thread").unwrap();
        select_first_word(&mut harness, card, "because the lock");
        crate::review::cards::more::pick(&mut harness, card, "Quote Reply");
        let composer = state.composer.get().expect("the Follow up box opened");
        assert_eq!(composer.thread_id.as_deref(), Some("t1"));
        assert!(
            !composer.destination.is_github(),
            "{:?}",
            composer.destination
        );
        assert_eq!(state.composer_draft.get(), "> because\n\n");

        // Nothing selected now: the whole answer, under what was typed.
        state.composer_draft.set("> because\n\nWhy?".into());
        let card = find(harness.root_id(), "card:thread").unwrap();
        crate::review::cards::more::pick(&mut harness, card, "Quote Reply");
        assert_eq!(
            state.composer_draft.get(),
            "> because\n\nWhy?\n\n> because the lock\n\n"
        );
        let composer = state.composer.get().unwrap();
        assert!(!composer.destination.is_github());
    }

    /// Conversation is the home for local threads that have no inline card:
    /// item-level threads, stale anchors, and anchors in another file.
    #[test]
    fn local_threads_without_an_inline_home_still_draw_on_conversation() {
        let (mut harness, _, state) = app_harness();
        let local_thread =
            |id: &str, body: &str, path: Option<&str>, outdated: bool| -> CommentThread {
                let mut thread = thread();
                thread.id = id.into();
                thread.anchor = match path {
                    Some(path) => {
                        let mut anchor = thread.anchor.take().unwrap();
                        anchor.path = path.into();
                        Some(anchor)
                    }
                    None => None,
                };
                thread.outdated = outdated;
                for (index, message) in thread.messages.iter_mut().enumerate() {
                    message.id = format!("{id}-m{index}");
                    message.thread_id = id.into();
                    message.run = None;
                }
                thread.messages[0].body = body.into();
                thread
            };
        state.comment_threads.set(Arc::new(vec![
            local_thread("item-level", "question about the whole item", None, false),
            local_thread(
                "outdated",
                "question about stale lines",
                Some("src/manifest.rs"),
                true,
            ),
            local_thread(
                "other-file",
                "question in another file",
                Some("src/elsewhere.rs"),
                false,
            ),
        ]));
        harness.rebuild();
        harness.paint();

        assert_eq!(
            cards_in_order(&harness)
                .iter()
                .filter(|card| card.as_str() == "card:thread")
                .count(),
            3
        );
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let text = all.join("\n");
        for expected in [
            "question about the whole item",
            "question about stale lines",
            "question in another file",
            "Outdated",
            "Label: \"src/elsewhere.rs\"",
        ] {
            assert!(text.contains(expected), "no {expected:?} on Conversation");
        }
    }

    #[test]
    fn a_bare_branch_says_so_under_the_composer() {
        let (mut harness, _, state) = app_harness();
        state.pull_request.set(None);
        state.discussion.set(None);
        state.comment_threads.set(Arc::new(vec![]));
        harness.rebuild();
        harness.paint();
        assert!(cards_in_order(&harness).is_empty());
        assert!(shown(&harness, "Nothing here yet"));
        assert!(shown(&harness, "Ask about this worktree"));
        let at = |text: &str| {
            find(harness.root_id(), text)
                .unwrap()
                .get_visual_rect_no_clip()
        };
        let (composer, empty) = (at("composer"), at("Nothing here yet"));
        assert!(
            composer.y1 <= empty.y0,
            "the label at {empty:?} is not under the composer at {composer:?}"
        );
    }

    #[test]
    fn a_bare_branch_lists_its_commits() {
        let (mut harness, _, state) = app_harness();
        state.item.update(|item| {
            if let Some(item) = item {
                Arc::make_mut(item).subject = None;
            }
        });
        state.pull_request.set(None);
        state.discussion.set(None);
        state.comment_threads.set(Arc::new(vec![]));
        state.commits.set(Arc::new(vec![perch_git::Commit {
            sha: "abcdef1234".into(),
            subject: "Mirror the manifest".into(),
            body: String::new(),
            author: "Chris".into(),
            authored_at: at("2026-09-18T09:00:00Z"),
        }]));
        harness.rebuild();
        harness.paint();
        assert_eq!(
            cards_in_order(&harness).len(),
            1,
            "{:?}",
            cards_in_order(&harness)
        );
        assert!(!shown(&harness, "Nothing here yet"));
    }

    #[test]
    fn the_composer_opens_in_place_and_a_follow_up_under_its_thread() {
        let (mut harness, _, state) = app_harness();
        state.open_composer(
            None,
            None,
            "why?".into(),
            (
                perch_agent::Backend::ClaudeCode,
                "sonnet".into(),
                perch_agent::Effort::Medium,
            ),
        );
        harness.rebuild();
        harness.paint();
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        assert!(
            all.iter().any(|n| n.contains("composer")),
            "the in-place composer is not in the tree"
        );
        assert!(
            state.composer_box.get().is_some(),
            "the box did not register itself for `a`"
        );

        // Picking a destination keeps the box: the container is keyed on
        // what the composer is about, not on the composer.
        let before = state.composer_box.get();
        state.composer.update(|c| {
            if let Some(c) = c {
                c.destination = crate::review::model::Destination::Agent {
                    backend: perch_agent::Backend::ClaudeCode,
                    model: "opus".into(),
                    effort: perch_agent::Effort::Medium,
                };
            }
        });
        harness.rebuild();
        harness.paint();
        assert_eq!(state.composer_box.get(), before);

        state.close_composer();
        harness.rebuild();
        harness.paint();
        assert!(
            state.composer_box.get().is_none(),
            "closing the composer left its box registered"
        );
    }

    #[test]
    fn the_style_buttons_edit_the_draft_and_preview_shows_it_read() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        state.open_composer(
            None,
            None,
            "why?".into(),
            (
                perch_agent::Backend::ClaudeCode,
                "sonnet".into(),
                perch_agent::Effort::Medium,
            ),
        );
        let backend = Rc::new(crate::backend::Backend::detached());
        let view = crate::review::threads::composer_view(
            app,
            state,
            backend,
            state.composer.get().unwrap(),
            ThreadFace::Card,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "composer-format:bold"));

        // The caret is after the draft, so Bold leaves empty marks to
        // type between.
        let bold = find(harness.root_id(), "composer-format:bold").unwrap();
        click_on(&mut harness, bold);
        assert_eq!(state.composer_draft.get(), "why?****");

        let preview = find(harness.root_id(), "composer-tab:Preview").unwrap();
        click_on(&mut harness, preview);
        assert!(shown(&harness, "composer-preview"));
        assert!(!shown(&harness, "composer editor box"));
        assert!(
            !shown(&harness, "composer-format:bold"),
            "no styles to put on in Preview"
        );

        let write = find(harness.root_id(), "composer-tab:Write").unwrap();
        click_on(&mut harness, write);
        assert!(!shown(&harness, "composer-preview"));
        assert!(shown(&harness, "composer editor box"));
        assert_eq!(state.composer_draft.get(), "why?****");

        // Cmd-B again, with the caret still between the marks, takes
        // them off; Cmd-Shift-P goes to Preview and back.
        let cmd = |code, c: &str, shift: bool| {
            let mut modifiers = crate::ui::OS_MOD;
            if shift {
                modifiers |= floem::prelude::Modifiers::SHIFT;
            }
            floem::event::Event::Key(floem::prelude::KeyboardEvent {
                state: floem::prelude::KeyState::Down,
                key: floem::prelude::Key::Character(c.into()),
                code,
                location: Default::default(),
                modifiers,
                repeat: false,
                is_composing: false,
            })
        };
        harness.dispatch_event(cmd(floem::prelude::Code::KeyB, "b", false));
        harness.rebuild();
        assert_eq!(state.composer_draft.get(), "why?");
        harness.dispatch_event(cmd(floem::prelude::Code::KeyP, "p", true));
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "composer-preview"));
        harness.dispatch_event(cmd(floem::prelude::Code::KeyP, "p", true));
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "composer-preview"));
    }

    /// Suggest is there on a composer on lines, puts the lines in a
    /// suggestion fence, and Preview draws the change against them; a
    /// composer on the item as a whole has no lines to suggest on.
    #[test]
    fn suggest_fences_the_lines_and_preview_draws_the_change() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\n    mut manifest,\nc\n".into(),
            "a\n    manifest,\nc\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        state.files.update(|m| {
            m.insert("src/x.rs".into(), Arc::new(file));
        });
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        let anchor = Anchor {
            path: "src/x.rs".into(),
            start_line: 2,
            line: 2,
            side: Side::Right,
            commit_sha: "h".into(),
        };
        state.open_composer(None, Some(anchor), String::new(), choice.clone());
        let backend = Rc::new(crate::backend::Backend::detached());
        let view = crate::review::threads::composer_view(
            app,
            state,
            backend,
            state.composer.get().unwrap(),
            ThreadFace::Card,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        assert!(shown(&harness, "composer-suggest"));

        let suggest = find(harness.root_id(), "composer-suggest").unwrap();
        click_on(&mut harness, suggest);
        assert_eq!(
            state.composer_draft.get(),
            "```suggestion\n    mut manifest,\n```"
        );

        let preview = find(harness.root_id(), "composer-tab:Preview").unwrap();
        click_on(&mut harness, preview);
        assert!(shown(&harness, "suggested-change"));

        // On the item as a whole there are no lines to suggest on.
        state.open_composer(None, None, String::new(), choice);
        harness.rebuild();
        harness.paint();
        assert!(!shown(&harness, "composer-suggest"));
    }

    /// A comment from GitHub with a suggestion in it is drawn as the
    /// change, against the lines its hunk ends on.
    #[test]
    fn a_suggestion_from_github_is_drawn_against_its_hunk() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let mut thread = thread();
        thread.anchor = Some(Anchor {
            path: "src/manifest.rs".into(),
            start_line: 40,
            line: 40,
            side: Side::Right,
            commit_sha: "h".into(),
        });
        thread.messages.truncate(1);
        let message = &mut thread.messages[0];
        message.origin = ThreadOrigin::GitHub;
        message.author = Author::GitHubUser("rohan".into());
        message.github_id = Some(7);
        message.run = None;
        message.body = "```suggestion\nlet c = 3;\n```".into();
        message.diff_hunk = Some("@@ -39,2 +39,2 @@\n a\n-b\n+let c = 2;".into());
        state.comment_threads.set(Arc::new(vec![thread]));
        let backend = Rc::new(crate::backend::Backend::detached());
        let view = crate::review::threads::messages_view(
            app,
            state,
            backend,
            "t1".into(),
            ThreadFace::Card,
        );
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 600.0);
        harness.rebuild();
        harness.paint();
        let change = find(harness.root_id(), "suggested-change").expect("drawn as a change");
        // The header, then the line it replaces and the line it suggests.
        let rows = change.children()[1].children().len();
        assert_eq!(rows, 2);
    }

    /// The thread views build for Conversation and inline, from the same thread.
    #[test]
    fn a_thread_draws_with_both_faces() {
        use floem::views::Stack;
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        state.comment_threads.set(Arc::new(vec![thread()]));
        let backend = Rc::new(crate::backend::Backend::detached());
        let open_file: crate::review::OpenFile = Rc::new(|_| {});
        let view = Stack::vertical((
            crate::review::threads::messages_view(
                app,
                state,
                backend.clone(),
                "t1".into(),
                ThreadFace::Card,
            ),
            crate::review::threads::messages_view(
                app,
                state,
                backend.clone(),
                "t1".into(),
                ThreadFace::Inline,
            ),
            crate::review::threads::follow_up(
                app,
                state,
                backend.clone(),
                "t1".into(),
                ThreadFace::Card,
            ),
            crate::review::threads::follow_up(
                app,
                state,
                backend.clone(),
                "t1".into(),
                ThreadFace::Inline,
            ),
            crate::review::cards::thread_card(
                app,
                state,
                backend.clone(),
                open_file.clone(),
                thread(),
            ),
            crate::review::threads::thread_card(app, state, backend, thread()),
        ));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 1600.0);
        harness.rebuild();
        harness.paint();
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let text = all.join("\n");
        assert_eq!(
            text.matches("Worked for 14s").count(),
            8,
            "each face folds the run's steps to one line"
        );
        assert_eq!(text.matches("Follow up…").count(), 4);
        // Conversation's header names the file, as github.com's does; the
        // card in the code is under its lines, and names them.
        assert_eq!(
            text.matches("Label: \"src/manifest.rs\"").count(),
            1,
            "Conversation's card names the file; the inline one does not"
        );
        assert_eq!(
            text.matches("Label: \"R41\"").count(),
            1,
            "the inline card is on line R41; Conversation's does not say"
        );
    }

    fn key_a() -> floem::event::Event {
        floem::event::Event::Key(floem::prelude::KeyboardEvent {
            state: floem::prelude::KeyState::Down,
            key: floem::prelude::Key::Character("a".into()),
            code: floem::prelude::Code::KeyA,
            location: Default::default(),
            modifiers: floem::prelude::Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    /// `a` on Conversation opens the composer in place, about the item as
    /// a whole, and leaves its box registered for the next `a` to focus.
    #[test]
    fn a_opens_the_in_place_composer_and_leaves_its_box_for_the_next_press() {
        let (mut harness, _, state) = app_harness();
        // A bare letter goes to the focused view, and a fresh window has
        // none; a click on the page is what a reader would have done.
        harness.click(700.0, 500.0);
        harness.rebuild();
        // A selection left over from Code does not anchor it.
        state.selection.set(Some((3, 5)));
        harness.dispatch_event(key_a());
        harness.rebuild();
        harness.paint();
        let composer = state.composer.get().expect("`a` did not open the composer");
        assert_eq!((composer.thread_id, composer.anchor), (None, None));
        assert!(state.composer_box.get().is_some());
    }

    /// The poll's status, which the main pane's key reads, must not build
    /// the page again: the reader's place on it is in the scroll, and a
    /// new scroll starts at the top.
    #[test]
    fn a_poll_leaves_the_page_where_it_was_scrolled_to() {
        use crate::backend::{Event, Status};

        let (mut harness, app, _) = app_harness();
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let viewport = scroll.get_visual_rect_no_clip();
        let column = find(harness.root_id(), "conversation column").unwrap();
        let top = column.get_visual_rect_no_clip().y0;
        harness.scroll_down(viewport.center().x, viewport.center().y, 300.0);
        harness.rebuild();
        harness.paint();
        let scrolled = column.get_visual_rect_no_clip().y0;
        assert!(
            scrolled < top,
            "the page did not scroll: {top} then {scrolled}"
        );

        app.apply(Event::Status(Status::Refreshing));
        app.apply(Event::Status(Status::Idle {
            at: at("2026-09-18T10:00:00Z"),
            source: "GITHUB_TOKEN".into(),
        }));
        harness.rebuild();
        harness.paint();

        let after = find(harness.root_id(), "conversation").unwrap();
        assert_eq!(after, scroll, "the poll built the page again");
        assert_eq!(
            column.get_visual_rect_no_clip().y0,
            scrolled,
            "the poll moved the page"
        );
    }

    /// What a notification has the poll fetch lands on the page as it
    /// is: a comment said while the reader is on it is one more card in
    /// the same scroll, over the newest, and the reader is where they
    /// were: the card they had in view stays where it was in the window,
    /// the page scrolling down by what landed over it. The card that was
    /// newest moves down without being built again.
    #[test]
    fn a_comment_that_arrives_joins_the_page_in_place() {
        use crate::backend::Event;

        let (mut harness, app, state) = app_harness();
        harness.set_size(1400.0, 400.0);
        settle(&mut harness);
        let scroll = find(harness.root_id(), "conversation").unwrap();
        // The reader down at the oldest card, with newer ones over it.
        let oldest = *all_named(harness.root_id(), "card:event").last().unwrap();
        to_top(&mut harness, oldest);
        let in_view = oldest.get_visual_rect_no_clip().y0;
        let scrolled = state.conversation_scroll.get_untracked();
        assert!(scrolled > 0.0, "the page did not scroll");
        let before = cards_in_order(&harness);
        let newest = card_views(&harness)[1];
        assert!(newest.debug_name().contains("card:event"));

        let mut discussion = discussion();
        discussion.comments.push(IssueComment {
            id: 2,
            node_id: String::new(),
            user: "rohan".into(),
            user_avatar_url: String::new(),
            body: "One more thing.".into(),
            created_at: at("2026-09-18T09:00:00Z"),
            updated_at: None,
            html_url: "https://github.com/criccomini/perch/pull/57#issuecomment-2".into(),
            reactions: Default::default(),
            viewer_can_delete: false,
            viewer_can_update: false,
        });
        app.apply_with_review(
            Event::DiscussionReady {
                item_id: "wt".into(),
                discussion: Arc::new(discussion),
            },
            state,
        );
        settle(&mut harness);

        let mut expected = before.clone();
        expected.insert(1, "card:comment".into());
        assert_eq!(
            cards_in_order(&harness),
            expected,
            "the comment is not a new card"
        );
        assert_eq!(
            find(harness.root_id(), "conversation").unwrap(),
            scroll,
            "the comment built the page again"
        );
        assert_eq!(
            oldest.get_visual_rect_no_clip().y0,
            in_view,
            "the comment moved the card in view"
        );
        let comment = find(harness.root_id(), "card:comment").unwrap();
        assert!(
            state.conversation_scroll.get_untracked()
                >= scrolled + comment.get_visual_rect_no_clip().height(),
            "the page did not scroll by what landed over the reader"
        );
        assert_eq!(
            card_views(&harness)[2],
            newest,
            "the card that was newest was built again"
        );
        assert_eq!(
            newest.get_visual_rect_no_clip().y0 - comment.get_visual_rect_no_clip().y1,
            crate::theme::EVENT_GAP,
            "the card that was newest has not the room under a comment"
        );
    }

    /// At the page's top, what lands over the timeline is what the reader
    /// should see: the page stays at its top.
    #[test]
    fn a_comment_that_arrives_at_the_top_of_the_page_does_not_scroll_it() {
        let (mut harness, app, state) = app_harness();
        harness.set_size(1400.0, 400.0);
        settle(&mut harness);
        let mut more = discussion();
        more.comments.push(IssueComment {
            id: 2,
            node_id: String::new(),
            user: "rohan".into(),
            user_avatar_url: String::new(),
            body: "One more thing.".into(),
            created_at: at("2026-09-18T09:00:00Z"),
            updated_at: None,
            html_url: "https://github.com/criccomini/perch/pull/57#issuecomment-2".into(),
            reactions: Default::default(),
            viewer_can_delete: false,
            viewer_can_update: false,
        });
        deliver(app, state, more);
        settle(&mut harness);
        assert_eq!(state.conversation_scroll.get_untracked(), 0.0);
    }

    /// A file's tab showing takes the page away, as a link to a file from
    /// a card does; showing Conversation again builds the page scrolled to
    /// where the reader left it, not at its top.
    #[test]
    fn the_page_opens_where_it_was_left_after_a_files_tab() {
        use crate::review::model::ItemTab;

        let (mut harness, _, state) = app_harness();
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let viewport = scroll.get_visual_rect_no_clip();
        let root = harness.root_id();
        let column = move || find(root, "conversation column").unwrap();
        let top = column().get_visual_rect_no_clip().y0;
        harness.scroll_down(viewport.center().x, viewport.center().y, 300.0);
        harness.rebuild();
        harness.paint();
        let scrolled = column().get_visual_rect_no_clip().y0;
        assert!(scrolled < top, "the page did not scroll");

        state.show_tab(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        assert!(
            find(harness.root_id(), "conversation").is_none(),
            "the page is still up under the file's tab"
        );

        state.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        assert_ne!(find(harness.root_id(), "conversation").unwrap(), scroll);
        assert_eq!(
            column().get_visual_rect_no_clip().y0,
            scrolled,
            "the page opened somewhere else"
        );
    }

    /// A key that moves through the page, as the window receives it:
    /// Page Up and Page Down bare, the arrows with Cmd.
    fn page_key(paging: Paging) -> floem::event::Event {
        use floem::prelude::{Code, Key, Modifiers, NamedKey};
        let (code, key, modifiers) = match paging {
            Paging::Up => (Code::PageUp, NamedKey::PageUp, Modifiers::empty()),
            Paging::Down => (Code::PageDown, NamedKey::PageDown, Modifiers::empty()),
            Paging::Top => (Code::ArrowUp, NamedKey::ArrowUp, crate::ui::OS_MOD),
            Paging::End => (Code::ArrowDown, NamedKey::ArrowDown, crate::ui::OS_MOD),
        };
        floem::event::Event::Key(floem::prelude::KeyboardEvent {
            state: floem::prelude::KeyState::Down,
            key: Key::Named(key),
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    /// Cmd-Down goes to the end of the page, the oldest card at the foot
    /// of the scroll, and Cmd-Up back to its top. A composer keeps them.
    #[test]
    fn cmd_up_and_down_go_to_the_end_of_the_page_and_back_to_its_top() {
        let (mut harness, _, state) = app_harness();
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let viewport = scroll.get_visual_rect_no_clip();
        let column = find(harness.root_id(), "conversation column").unwrap();
        let top = column.get_visual_rect_no_clip().y0;
        harness.click(700.0, 500.0);
        harness.rebuild();

        harness.dispatch_event(page_key(Paging::End));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.paging.get(), None, "the page took the key");
        let foot = column.get_visual_rect_no_clip();
        // To a hundredth: the column's height is a sum of f32 rows.
        assert!(
            (foot.y1 - (viewport.y1 - crate::theme::CONVERSATION_PADDING)).abs() < 0.01,
            "the column's end at {foot:?} is not at the end of the scroll {viewport:?}"
        );
        let end = column.get_visual_rect_no_clip().y0;
        assert!(end < top);

        // A composer's are the start and the end of its text.
        state.composer_focused.set(true);
        harness.dispatch_event(page_key(Paging::Top));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.paging.get(), None);
        assert_eq!(column.get_visual_rect_no_clip().y0, end);
        state.composer_focused.set(false);

        harness.dispatch_event(page_key(Paging::Top));
        harness.rebuild();
        harness.paint();
        assert_eq!(column.get_visual_rect_no_clip().y0, top);
    }

    /// The ask box at the foot of the window opens into a composer taller
    /// than itself, and the page follows it down: the whole composer,
    /// Send and all, is on screen, not only its first lines.
    #[test]
    fn the_composer_opened_at_the_foot_of_the_window_is_all_on_screen() {
        let (mut harness, _, state) = app_harness();
        // The window cut under the ask box, with the room the composer
        // needs to fit once the page has followed it.
        let foot = find(harness.root_id(), "composer").unwrap();
        harness.set_size(1400.0, foot.get_visual_rect_no_clip().y1 + 100.0);
        harness.rebuild();
        harness.paint();
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let viewport = scroll.get_visual_rect_no_clip();

        click_on(&mut harness, foot);
        harness.rebuild();
        harness.paint();
        assert!(state.composer.get().is_some(), "the composer opened");
        let composer = find(harness.root_id(), "composer-box")
            .unwrap()
            .get_visual_rect_no_clip();
        assert!(
            composer.y0 >= viewport.y0 && composer.y1 <= viewport.y1 + 0.01,
            "the composer at {composer:?} is not all in the page {viewport:?}"
        );
    }

    /// Any text box on the page keeps Cmd-Up and Cmd-Down, not only the
    /// composer: the close-as-issue form's title and body do too.
    #[test]
    fn a_text_box_keeps_cmd_up_and_down() {
        let (mut harness, app, state) = app_harness();
        crate::review::close_as_issue::unfold(app, state, "wt");
        harness.rebuild();
        harness.paint();
        let root = harness.root_id();
        let column = move || find(root, "conversation column").unwrap();
        let top = column().get_visual_rect_no_clip().y0;
        let form = find(root, "close-as-issue").expect("no form");
        let boxes = [
            find(form, "TextInput").expect("no title box"),
            find(form, "Editor View").expect("no body box"),
        ];
        for text_box in boxes {
            text_box.request_focus();
            harness.rebuild();
            assert!(harness.is_focused(text_box));
            harness.dispatch_event(page_key(Paging::End));
            harness.rebuild();
            harness.paint();
            assert_eq!(state.paging.get(), None);
            assert_eq!(
                column().get_visual_rect_no_clip().y0,
                top,
                "{} gave the page the key",
                text_box.debug_name()
            );
        }
    }

    /// Page Down scrolls a page less a line, and Page Up back; from a
    /// composer as well, as its few lines are not what a reader pages
    /// through.
    #[test]
    fn the_page_keys_scroll_by_a_page() {
        let (mut harness, _, state) = app_harness();
        // Short enough that the page is over two pages long.
        harness.set_size(1400.0, 240.0);
        harness.rebuild();
        harness.paint();
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let page =
            scroll.get_content_rect_local().height() - crate::theme::code_line_height() as f64;
        let column = find(harness.root_id(), "conversation column").unwrap();
        let top = column.get_visual_rect_no_clip().y0;
        // On the page beside the column, where a click opens nothing.
        harness.click(460.0, 200.0);
        harness.rebuild();

        harness.dispatch_event(page_key(Paging::Down));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.paging.get(), None, "the page took the key");
        assert_eq!(state.conversation_scroll.get(), page);
        harness.dispatch_event(page_key(Paging::Down));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.conversation_scroll.get(), 2.0 * page);
        assert_eq!(column.get_visual_rect_no_clip().y0, top - 2.0 * page);

        state.composer_focused.set(true);
        harness.dispatch_event(page_key(Paging::Up));
        harness.rebuild();
        harness.paint();
        assert_eq!(state.conversation_scroll.get(), page);
    }

    /// The page keys work before the page is clicked: floem hands them
    /// only to what has the focus, so the page takes it as it comes up,
    /// the first time and again in place of the code.
    #[test]
    fn the_page_keys_work_on_a_page_nobody_clicked() {
        let (mut harness, _, state) = app_harness();
        let root = harness.root_id();
        let column = move || find(root, "conversation column").unwrap();
        let top = column().get_visual_rect_no_clip().y0;
        harness.dispatch_event(page_key(Paging::Down));
        harness.rebuild();
        harness.paint();
        assert!(
            column().get_visual_rect_no_clip().y0 < top,
            "the first page did not move"
        );

        harness.dispatch_event(page_key(Paging::Up));
        harness.rebuild();
        harness.paint();
        state.show_tab(crate::review::model::ItemTab::Code);
        harness.rebuild();
        harness.paint();
        state.show_tab(crate::review::model::ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        assert_eq!(column().get_visual_rect_no_clip().y0, top);
        harness.dispatch_event(page_key(Paging::Down));
        harness.rebuild();
        harness.paint();
        assert!(
            column().get_visual_rect_no_clip().y0 < top,
            "the page back from Code did not move"
        );
    }

    /// The notification about pull request 57, dated `updated_at`.
    fn notification(updated_at: &str, unread: bool) -> perch_github::Thread {
        use perch_github::{Reason, Repository, Subject, SubjectKind, Thread};
        Thread {
            id: "n57".into(),
            unread,
            local_unread: false,
            local_read: false,
            reason: Reason::Comment,
            updated_at: at(updated_at),
            last_read_at: Some(at("2026-09-16T00:00:00Z")),
            subject: Subject {
                title: "Fence the manifest".into(),
                kind: SubjectKind::PullRequest,
                url: Some("https://api.github.com/repos/criccomini/perch/pulls/57".into()),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "criccomini/perch".into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
            snoozed_until: None,
        }
    }

    /// The fixture's discussion, with the comment the notification is
    /// about said after the merge.
    fn with_new_comment() -> Discussion {
        let mut discussion = discussion();
        discussion.comments.push(IssueComment {
            id: 2,
            node_id: String::new(),
            user: "rohan".into(),
            user_avatar_url: String::new(),
            body: "One more thing.".into(),
            created_at: at("2026-09-18T09:00:00Z"),
            updated_at: None,
            html_url: "https://github.com/criccomini/perch/pull/57#issuecomment-2".into(),
            reactions: Default::default(),
            viewer_can_delete: false,
            viewer_can_update: false,
        });
        discussion
    }

    /// The window, short enough that the page is longer than it, with an
    /// open of `thread` followed from its first paint.
    fn opened_from(thread: perch_github::Thread) -> (HeadlessHarness, AppState, ReviewState) {
        let (mut harness, app, state) = app_harness_prepared(
            crate::backend::Backend::detached(),
            discussion(),
            move |state| crate::review::notified::follow(state, &thread),
        );
        harness.set_size(1400.0, 320.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
    }

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
    }

    /// Whether all of the newest card of `kind` on the page, the first,
    /// or else the oldest, the last, is inside the scroll's box.
    fn card_shown(harness: &HeadlessHarness, kind: &str, newest: bool) -> bool {
        let found = all_named(harness.root_id(), kind);
        let card = if newest { found.first() } else { found.last() }
            .expect("no such card")
            .get_visual_rect_no_clip();
        let viewport = find(harness.root_id(), "conversation")
            .unwrap()
            .get_visual_rect_no_clip();
        viewport.contains(card.origin()) && card.y1 <= viewport.y1 + 0.5
    }

    fn deliver(app: AppState, state: ReviewState, discussion: Discussion) {
        app.apply_with_review(
            crate::backend::Event::DiscussionReady {
                item_id: "wt".into(),
                discussion: Arc::new(discussion),
            },
            state,
        );
    }

    /// The page painted from the cache waits; GitHub's answer brings the
    /// comment that woke the reader, and the page goes to it.
    #[test]
    fn an_unread_notification_opens_on_the_comment_that_woke_the_reader() {
        let (mut harness, app, state) = opened_from(notification("2026-09-18T09:00:20Z", true));
        let column = find(harness.root_id(), "conversation column").unwrap();
        let top = column.get_visual_rect_no_clip().y0;
        assert_eq!(state.focus_card.get(), None, "the cached page is older");

        deliver(app, state, with_new_comment());
        settle(&mut harness);

        assert_eq!(
            state.focus_card.get(),
            Some(crate::review::doc::CardId::Entry("comment:2".into()))
        );
        assert!(
            column.get_visual_rect_no_clip().y0 < top,
            "the page did not move"
        );
        assert!(
            card_shown(&harness, "card:comment", true),
            "the new comment is not in view"
        );
        assert!(state.notified.get().is_none(), "the open is done");
    }

    /// What woke the reader is not always on the page: after the wait,
    /// the newest card there is.
    #[test]
    fn a_wait_out_goes_to_the_newest_card_the_page_has() {
        let (mut harness, _, state) = opened_from(notification("2026-09-18T09:00:20Z", true));
        crate::review::notified::settle(state, &super::cards(state), true);
        settle(&mut harness);
        assert!(
            matches!(
                state.focus_card.get(),
                Some(crate::review::doc::CardId::Entry(id)) if id.starts_with("event:merged")
            ),
            "{:?}",
            state.focus_card.get()
        );
        assert!(card_shown(&harness, "card:event", true));
    }

    /// A read notification opens where the reader left the page, and so
    /// does one the reader scrolled before GitHub answered.
    #[test]
    fn a_read_notification_or_a_scroll_first_leaves_the_page_alone() {
        let (mut harness, app, state) = opened_from(notification("2026-09-18T09:00:20Z", false));
        deliver(app, state, with_new_comment());
        settle(&mut harness);
        assert_eq!(state.focus_card.get(), None);

        let (mut harness, app, state) = opened_from(notification("2026-09-18T09:00:20Z", true));
        let viewport = find(harness.root_id(), "conversation")
            .unwrap()
            .get_visual_rect_no_clip();
        harness.scroll_down(viewport.center().x, viewport.center().y, 40.0);
        deliver(app, state, with_new_comment());
        settle(&mut harness);
        assert_eq!(state.focus_card.get(), None);
        assert!(state.notified.get().is_none());
    }

    /// An item last left on Code opens on Conversation for the update.
    #[test]
    fn an_unread_notification_opens_on_conversation_from_code() {
        let thread = notification("2026-09-18T09:00:20Z", true);
        let (mut harness, app, state) = app_harness_prepared(
            crate::backend::Backend::detached(),
            discussion(),
            move |state| {
                state.show_tab(crate::review::model::ItemTab::Code);
                crate::review::notified::follow(state, &thread);
            },
        );
        harness.set_size(1400.0, 500.0);
        settle(&mut harness);
        assert!(find(harness.root_id(), "conversation").is_none());

        deliver(app, state, with_new_comment());
        settle(&mut harness);
        assert_eq!(state.tab.get(), crate::review::model::ItemTab::Conversation);
        assert!(
            card_shown(&harness, "card:comment", true),
            "the new comment is not in view"
        );
    }
}
