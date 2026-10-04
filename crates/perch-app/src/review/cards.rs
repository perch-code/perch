//! The entries on Conversation: the description, each comment and review
//! from GitHub, a line for each event, and each thread, the reader's own
//! and GitHub's review threads alike.
//!
//! Everything said is a card, and every card is one of two boxes: white
//! and framed for what is on GitHub, Codex's grey for what only Perch
//! has (`github_surface`, `perch_surface`). What kind of thing a card is,
//! a comment, a review, a thread on a line, is said by its header, never
//! by its box. What happened, a push or a merge, is a line between the
//! cards with no box at all.
//!
//! A local thread and a GitHub thread must never look like one: nothing
//! local reaches GitHub, so a local thread is grey and its header says
//! Perch only, and the reader's asks inside a GitHub thread are a grey
//! band across its white.
//!
//! What a GitHub entry does carry of the reader's is its reactions: the
//! pills under its text, and the face in its byline that opens the
//! picker. A click toggles the pill where it is and asks GitHub after.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use floem::context::VisualChanged;
use floem::event::EventPropagation;
use floem::kurbo::{Rect, Size};
use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, Overlay, Stack, TooltipClass,
    TooltipExt,
};
use perch_git::{LineStatus, Row};
use perch_github::{
    Anchor, CommentThread, IssueComment, PullEvent, PullEventKind, ReactionGroup, ReactionKind,
    Review,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::markdown;
use crate::review::model::{Destination, ItemTab, LoadedFile, ReviewState};
use crate::review::threads::{self, ThreadFace};
use crate::review::timeline::{count_of, event_name, event_pieces, short_sha, verdict, Card, Piece};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{
    avatar, clickable, confirmation, follow_link, name_label, open_in_browser, sidebar_glyph,
    underlined_link, Name,
};

/// What a card's row in the column is keyed by. A row with the same key is
/// kept as it is, so the key changes with anything the card shows that
/// GitHub can edit; a thread's messages are read live inside its card,
/// and the key carries their count, which is last, and the first's body,
/// which a collapsed row shows. It carries the anchor too: GitHub moves a
/// comment's line as commits land above it, and the card names the line.
pub fn key(card: &Card) -> String {
    match card {
        // The time lands with the discussion, after a first paint from
        // the snapshot, and an edit can keep a body's length.
        Card::Description { body, at, editable, .. } => {
            format!("description:{:?}:{:x}:{}", at, body_hash(body), editable)
        }
        // An edit made here shows before GitHub dates it, and what the
        // reader may do with it lands after a first paint.
        Card::Comment(c) => format!(
            "comment:{}:{:?}:{:x}:{}:{}",
            c.id,
            c.updated_at,
            body_hash(&c.body),
            c.viewer_can_update,
            c.viewer_can_delete
        ),
        Card::Review { review } => {
            format!(
                "review:{}:{:?}:{:x}:{}",
                review.id,
                review.submitted_at,
                body_hash(&review.body),
                review.viewer_can_update
            )
        }
        Card::Event(e) => format!("event:{}:{}", event_name(e), e.at),
        Card::Thread(t) => format!(
            "thread:{}:{}:{}:{}",
            t.id,
            t.outdated,
            t.resolved,
            thread_key_tail(t)
        ),
    }
}

/// The part of a thread's key that both faces share: the anchor, the
/// message count, the last message's id and the first's body.
pub(crate) fn thread_key_tail(t: &CommentThread) -> String {
    let anchor = t
        .anchor
        .as_ref()
        .map(|a| {
            format!(
                "{}:{}:{}:{}:{}",
                a.path,
                a.start_line,
                a.line,
                a.side.name(),
                a.commit_sha
            )
        })
        .unwrap_or_default();
    format!(
        "{}:{}:{}:{:x}",
        anchor,
        t.messages.len(),
        t.messages.last().map(|m| m.id.as_str()).unwrap_or_default(),
        t.messages.first().map(|m| body_hash(&m.body)).unwrap_or_default()
    )
}

/// A body's hash, for a key that must follow an edit which keeps the
/// body's length.
pub(crate) fn body_hash(body: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------
// Chrome
// ---------------------------------------------------------------------

/// A box of what is on GitHub: white, framed.
pub fn github_surface(s: floem::style::Style) -> floem::style::Style {
    s.background(theme::canvas())
        .border(1.0)
        .border_color(theme::border())
        .border_radius(theme::CARD_RADIUS)
}

/// A box of what only Perch has: Codex's grey, framed a step quieter.
pub fn perch_surface(s: floem::style::Style) -> floem::style::Style {
    s.background(theme::perch_bg())
        .border(1.0)
        .border_color(theme::border_muted())
        .border_radius(theme::CARD_RADIUS)
}

/// The header of a section after the timeline, Checks, Agents or
/// Reviews: one height and the cards' room at its sides on each, its
/// title first and a fold's chevron, if it has one, last.
pub fn section_header_style(s: floem::style::Style) -> floem::style::Style {
    s.items_center()
        .width_full()
        .height(theme::SECTION_HEADER_HEIGHT)
        .padding_horiz(theme::CARD_PADDING)
        .gap(8.0)
}

/// A section's title, set as a byline's name is but a step larger.
pub fn section_title(text: &'static str) -> Label {
    Label::derived(move || text.to_string()).style(|s| {
        s.font_size(theme::TEXT_BODY)
            .font_weight(theme::weight_emphasis())
            .color(theme::fg())
            .flex_shrink(0.0_f32)
            .selectable(false)
    })
}

/// One GitHub entry on Conversation: its byline, then its text, if it
/// has any, in a white box.
pub fn card(header: impl IntoView + 'static, body: Option<floem::AnyView>) -> impl IntoView {
    let body = match body {
        Some(body) => Container::new(body)
            .style(|s| s.width_full().padding_top(4.0).padding_bottom(8.0))
            .into_any(),
        None => Empty::new().into_any(),
    };
    Stack::vertical((header, body)).style(|s| {
        github_surface(s.width_full().flex_col())
            .padding_horiz(theme::CARD_PADDING)
            .padding_vert(6.0)
    })
}

/// The byline row at the top of an entry.
fn byline_strip(content: impl IntoView + 'static) -> impl IntoView {
    Container::new(content).style(|s| {
        s.items_center()
            .width_full()
            .height(theme::BYLINE_HEIGHT)
    })
}

/// Who, with what glyph, did what, when. The avatar is read when it is
/// drawn, so a byline built before the discussion has landed can still
/// find its picture in it.
pub(crate) struct Byline {
    pub(crate) login: String,
    pub(crate) avatar_url: Box<dyn Fn() -> String>,
    pub(crate) avatar_size: f64,
    /// A verdict's glyph between the avatar and the login.
    pub(crate) glyph: Option<(SidebarIcon, fn() -> Color)>,
    pub(crate) verb: String,
    pub(crate) when: Option<jiff::Timestamp>,
}

/// Avatar, login, what they did and when, and whatever the caller puts at
/// the right: its actions.
pub(crate) fn byline(app: AppState, who: Byline, trailing: floem::AnyView) -> impl IntoView {
    let Byline {
        login,
        avatar_url,
        avatar_size,
        glyph,
        verb,
        when,
    } = who;
    let seed = login.clone();
    let verb_empty = verb.is_empty();
    // A verdict's words take its glyph's colour, so Changes requested
    // reads as red as its mark does; a comment's are muted.
    let verb_color = glyph.map(|(_, color)| color);
    // `now` ticks each second while an agent runs; a memo only tells the
    // label when the words change.
    let ago = Memo::new(move |_| {
        when.map(|at| timefmt::relative(at, app.now.get()))
            .unwrap_or_default()
    });
    Stack::horizontal((
        avatar(app, avatar_url, avatar_size, seed)
            .style(|s| s.margin_right(9.0).flex_shrink(0.0_f32)),
        match glyph {
            Some((icon, color)) => sidebar_glyph(icon, color)
                .style(|s| s.size(14.0, 14.0).margin_right(5.0))
                .into_any(),
            None => Empty::new().into_any(),
        },
        name_label(
            move || Name::Login(login.clone()),
            |s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
            },
        ),
        Label::derived(move || verb.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(verb_color.map_or_else(theme::fg_muted, |color| color()))
                .margin_left(6.0)
                .flex_shrink(0.0_f32)
                .apply_if(verb_empty, |s| s.hide())
        }),
        Label::derived(move || ago.get()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .margin_left(if verb_empty { 6.0 } else { 4.0 })
                .text_ellipsis()
                .min_width(0.0)
                .apply_if(when.is_none(), |s| s.hide())
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        trailing,
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// The right end of a GitHub byline, a glyph each and all only while the
/// card is hovered: whatever the caller puts first, a Reply on a review
/// comment or Quote reply on a card; Ask about this, the reaction picker
/// and the permalink. `react` is the subject's node id; an entry an older
/// build kept without one has no picker, since GitHub could not be told
/// what it was for.
pub(crate) fn actions(
    state: ReviewState,
    backend: Rc<Backend>,
    hovered: RwSignal<bool>,
    leading: Option<floem::AnyView>,
    ask: Option<Rc<dyn Fn()>>,
    react: Option<String>,
    url: String,
) -> floem::AnyView {
    Stack::horizontal((
        match leading {
            Some(leading) => leading,
            None => Empty::new().into_any(),
        },
        match ask {
            Some(ask) => hover_action(hovered, SidebarIcon::Thread, "Ask about this", "ask-about", ask)
                .into_any(),
            None => Empty::new().into_any(),
        },
        match react {
            Some(node_id) => react_button(state, backend, node_id, Some(hovered)).into_any(),
            None => Empty::new().into_any(),
        },
        if url.is_empty() {
            Empty::new().into_any()
        } else {
            hover_action(
                hovered,
                SidebarIcon::Link,
                "Open on GitHub",
                "github-link",
                Rc::new(move || open_in_browser(&url)),
            )
            .style(|s| s.margin_right(-4.0))
            .into_any()
        },
    ))
    .style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32))
    .into_any()
}

/// A glyph that does `action`, named by its tip, "Ask about this" or
/// "Reply", drawn only while the card is under the pointer. Hidden rather
/// than absent, so the byline keeps its shape.
pub(crate) fn hover_action(
    hovered: RwSignal<bool>,
    icon: SidebarIcon,
    tip_text: &'static str,
    name: &'static str,
    action: Rc<dyn Fn()>,
) -> impl IntoView {
    tip(
        Container::new(sidebar_glyph(icon, theme::fg_muted).style(|s| s.size(14.0, 14.0)))
            .on_event_stop(el::Click, move |_, _| action())
            .debug_name(name)
            .style(move |s| {
                clickable(s)
                    .size(24.0, 24.0)
                    .items_center()
                    .justify_center()
                    .border_radius(12.0)
                    .hover(|s| s.background(theme::badge_bg()))
                    .apply_if(!hovered.get(), |s| s.hide())
            }),
        tip_text.to_string(),
    )
}

/// Quote reply under the pointer, as [`hover_action`] is: `quote` gets
/// what was selected in `scope`'s text as the pointer came onto the
/// button, or `None` to quote the whole. A press moves the focus, and
/// the selection goes with it, before the click lands, so the selection
/// is read on the way in.
pub(crate) fn quote_action(
    hovered: RwSignal<bool>,
    scope: floem::ViewId,
    name: &'static str,
    quote: Rc<dyn Fn(Option<String>)>,
) -> impl IntoView {
    let selected = Rc::new(RefCell::new(None));
    let read = selected.clone();
    let action: Rc<dyn Fn()> = Rc::new(move || quote(read.borrow().clone()));
    Container::new(hover_action(hovered, SidebarIcon::Reply, "Quote reply", name, action))
        .on_event_cont(el::PointerEnter, move |_, _| {
            *selected.borrow_mut() = crate::review::linked_text::selected_under(scope);
        })
}

/// Delete under the pointer, for a comment on GitHub, while `may` says
/// GitHub lets the reader delete it; absent otherwise, as Quote reply is
/// where it has nowhere to go. It asks first, under the comment.
pub(crate) fn delete_action(
    state: ReviewState,
    hovered: RwSignal<bool>,
    node_id: String,
    may: impl Fn() -> bool + 'static,
) -> floem::AnyView {
    let delete = Rc::new(move || state.confirm_comment.set(Some(node_id.clone())));
    offered_action(hovered, may, SidebarIcon::Trash, "Delete", "delete-comment", delete)
}

/// Edit under the pointer, for a comment on GitHub, while `may` says
/// GitHub lets the reader edit it; absent otherwise. `edit` opens the
/// composer in the comment's place with its text.
pub(crate) fn edit_action(
    hovered: RwSignal<bool>,
    may: impl Fn() -> bool + 'static,
    edit: Rc<dyn Fn()>,
) -> floem::AnyView {
    offered_action(hovered, may, SidebarIcon::Pencil, "Edit", "edit-comment", edit)
}

/// A hover action that is there only while `may` says so.
fn offered_action(
    hovered: RwSignal<bool>,
    may: impl Fn() -> bool + 'static,
    icon: SidebarIcon,
    label: &'static str,
    name: &'static str,
    action: Rc<dyn Fn()>,
) -> floem::AnyView {
    let offered = Memo::new(move |_| may());
    dyn_container(
        move || offered.get(),
        move |offered| {
            if !offered {
                return Empty::new().into_any();
            }
            hover_action(hovered, icon, label, name, action.clone()).into_any()
        },
    )
    .into_any()
}

/// What a card puts first among its [`actions`], in a row: Quote reply or
/// a thread's Reply where there is one, Edit, then Delete where the card
/// offers it.
pub(crate) fn leading_actions(
    first: Option<floem::AnyView>,
    edit: floem::AnyView,
    delete: Option<floem::AnyView>,
) -> floem::AnyView {
    let first = first.unwrap_or_else(|| Empty::new().into_any());
    match delete {
        Some(delete) => Stack::horizontal((first, edit, delete)),
        None => Stack::horizontal((first, edit)),
    }
    .style(|s| s.items_center().gap(4.0))
    .into_any()
}

/// "Delete this comment?" with Delete and Cancel, under the comment that
/// asked. Delete takes the comment out of the view at once, a line in the
/// timeline in its place for one in the conversation, and asks GitHub to
/// delete it for good; GitHub's refusal puts it back.
pub(crate) fn delete_comment_confirmation(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
) -> impl IntoView {
    let asked = node_id.clone();
    let asking = Memo::new(move |_| {
        state
            .confirm_comment
            .with(|c| c.as_deref() == Some(asked.as_str()))
    });
    dyn_container(
        move || asking.get(),
        move |asking| {
            if !asking {
                return Empty::new().into_any();
            }
            let backend = backend.clone();
            let node_id = node_id.clone();
            confirmation(
                "delete-comment",
                "Delete this comment?".to_string(),
                "It goes from GitHub for good, for everyone.".to_string(),
                "Delete",
                move || {
                    state.confirm_comment.set(None);
                    let Some(item_id) =
                        state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone()))
                    else {
                        return;
                    };
                    let actor = app
                        .viewer
                        .get_untracked()
                        .map(|(login, _)| login)
                        .unwrap_or_default();
                    if let Some(place) = state.take_out_github_comment(&node_id, &actor) {
                        backend.delete_github_comment(&item_id, &node_id, place);
                    }
                },
                move || state.confirm_comment.set(None),
            )
            .style(|s| s.margin_top(8.0))
            .into_any()
        },
    )
    .style(move |s| s.width_full().apply_if(!asking.get(), |s| s.hide()))
}

/// Follow the pointer in and out of a card, for Ask about this.
///
/// Whether the pointer is over the view is read from floem's hover path
/// when either event arrives, rather than taken from which one it was.
/// A floem hover path holds only the views whose own boxes hold the
/// pointer, so when the pointer crosses the edge of an ancestor's box,
/// as the Submit review form hangs out of its button's, the path changes
/// above views still under the pointer. floem used to leave and re-enter
/// each of them then, the Leave last, which hid a draft's Delete as the
/// pointer reached it. The fork's `perch` branch sends them neither event
/// now; reading the path keeps this right whatever order events come in.
pub(crate) fn track_hover<V: IntoView + 'static>(view: V, hovered: RwSignal<bool>) -> impl IntoView {
    let view = view.into_intermediate();
    let id = view.view_id();
    let follow = move |cx: &mut floem::event::EventCx| {
        let over = cx.window_state.is_hovered(id);
        if hovered.get_untracked() != over {
            hovered.set(over);
        }
    };
    view.on_event_cont(el::PointerEnter, move |cx, _| follow(cx))
        .on_event_cont(el::PointerLeave, move |cx, _| follow(cx))
}

// ---------------------------------------------------------------------
// Reactions
// ---------------------------------------------------------------------

/// The node id a reaction is left by, when the entry has one.
pub(crate) fn reactable(node_id: &str) -> Option<String> {
    (!node_id.is_empty()).then(|| node_id.to_string())
}

/// The row of pills under an entry's text, for an entry with a node id;
/// nothing for one without, since nothing could be left on it.
pub(crate) fn reactions_under(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    react: &Option<String>,
) -> floem::AnyView {
    match react {
        Some(node_id) => reactions_row(app, state, backend, node_id.clone()).into_any(),
        None => Empty::new().into_any(),
    }
}

/// Toggle the reader's reaction where it is shown, then ask GitHub. A
/// pill already asked about takes no second click until GitHub answers.
fn react(state: ReviewState, backend: &Backend, node_id: &str, kind: ReactionKind) {
    let Some(item_id) = state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone())) else {
        return;
    };
    if let Some(add) = state.toggle_reaction(node_id, kind) {
        backend.set_reaction(&item_id, node_id, kind, add);
    }
}

/// A button in the picker, and the room between the buttons and around
/// them: GitHub's measures.
const PICK_SIZE: f64 = 32.0;
const PICK_GAP: f64 = 4.0;
/// A picked emoji is drawn larger than a pill's.
const PICK_EMOJI: f32 = 16.0;
/// The room between the face and the picker it opens.
const PICKER_OFFSET: f64 = 8.0;
/// The picker's size: the eight in a row, padded, inside a 1px rule.
const PICKER_WIDTH: f64 = ReactionKind::ALL.len() as f64 * (PICK_SIZE + PICK_GAP) + PICK_GAP + 2.0;
const PICKER_HEIGHT: f64 = PICK_SIZE + 2.0 * PICK_GAP + 2.0;

/// Where the picker opens from its face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placement {
    /// Over the face rather than under it.
    above: bool,
    /// From the face's left edge rightward rather than from its right
    /// edge leftward.
    rightward: bool,
}

impl Placement {
    /// The byline's face is at the top right of its entry, so the picker
    /// hangs under it and runs left; the row's is at the foot, after the
    /// pills, and the picker stands over it and runs right, as GitHub's
    /// does from the same place.
    fn preferred(in_byline: bool) -> Self {
        Placement {
            above: !in_byline,
            rightward: !in_byline,
        }
    }

    /// This placement for a face drawn at `face` in a window of `window`,
    /// turned on either axis where the picker would leave the window and
    /// the other way would not.
    fn fitted(self, face: Rect, window: Size) -> Self {
        let room_above = face.y0 - PICKER_OFFSET - PICKER_HEIGHT >= 0.0;
        let room_below = face.y1 + PICKER_OFFSET + PICKER_HEIGHT <= window.height;
        let room_right = face.x0 + PICKER_WIDTH <= window.width;
        let room_left = face.x1 - PICKER_WIDTH >= 0.0;
        Placement {
            above: if self.above {
                room_above || !room_below
            } else {
                !room_below && room_above
            },
            rightward: if self.rightward {
                room_right || !room_left
            } else {
                !room_left && room_right
            },
        }
    }
}

/// A face that opens the picker. In a byline it is drawn only while the
/// card is under the pointer, as its other actions are, and hidden rather
/// than absent so the byline keeps its shape; at the end of a row of
/// pills it is round and ruled like them, as GitHub's is, and always
/// there. Either stays drawn, pressed, while its picker is open.
///
/// The face takes the focus back however the picker closes: a click on
/// the backdrop focuses nothing, and a key must still find its way up
/// through the column to the window's handler.
fn react_button(
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
    hovered: Option<RwSignal<bool>>,
) -> impl IntoView {
    let in_byline = hovered.is_some();
    let (size, rule) = if in_byline {
        (24.0, 0.0)
    } else {
        (theme::REACTION_PILL_HEIGHT, 1.0)
    };
    let face_id = floem::ViewId::new();
    let open = RwSignal::new(None::<Placement>);
    let shown = Memo::new(move |_| open.get());
    let close: Rc<dyn Fn()> = Rc::new(move || {
        open.set(None);
        face_id.request_focus();
    });
    let toggle = move || {
        if open.get_untracked().is_some() {
            open.set(None);
            return;
        }
        // A byline's face the pointer has left keeps the focus it took,
        // hidden; Return on it must not open a picker from nowhere.
        if hovered.is_some_and(|h| !h.get_untracked()) {
            return;
        }
        let window = face_id.root().get_size().unwrap_or_default();
        let face = face_id.get_visual_rect_no_clip();
        open.set(Some(Placement::preferred(in_byline).fitted(face, window)));
    };
    // The picker hangs from the face's outer edge, its rule included.
    let slot = dyn_container(
        move || shown.get(),
        move |placement| match placement {
            Some(placement) => {
                picker(state, backend.clone(), node_id.clone(), placement, size, close.clone())
                    .into_any()
            }
            None => Empty::new().into_any(),
        },
    )
    .style(move |s| s.absolute().inset(-rule));
    tip(
        Stack::with_id(
            face_id,
            (
                sidebar_glyph(SidebarIcon::Smiley, theme::fg_muted)
                    .style(|s| s.size(14.0, 14.0).pointer_events_none()),
                slot,
            ),
        )
        .on_event_stop(el::Click, move |_, _| toggle())
        .style(move |s| {
            let open = shown.get().is_some();
            let s = clickable(s)
                .size(size, size)
                .items_center()
                .justify_center()
                .border_radius(size / 2.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()));
            let s = if in_byline {
                s.hover(|s| s.background(theme::badge_bg()))
                    .apply_if(hovered.is_some_and(|h| !h.get()) && !open, |s| s.hide())
            } else {
                s.border(rule)
                    .border_color(theme::border())
                    .background(theme::canvas())
                    .hover(|s| s.background(theme::badge_bg()).border_color(theme::fg_subtle()))
            };
            s.apply_if(open, |s| s.background(theme::badge_bg()))
        })
        .debug_name("react-button"),
        "React".into(),
    )
}

/// GitHub's picker: its eight in a row on a white card, each washed blue
/// while the reader's is among them, so picking it again takes it back.
/// A pick closes it, and so do Escape and a click anywhere else, which
/// nothing under the picker sees. It hangs from its face in the window's
/// overlay, where the column's scroll cannot clip it and nothing drawn
/// after the entry covers it, and follows the face if the column moves.
///
/// The first emoji takes the focus as it opens. The arrows and Tab move
/// it along the row, around from either end, and Home and End to the
/// ends; Return or Space picks. The focus never leaves the row while it
/// is open, so Escape always reaches it.
fn picker(
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
    placement: Placement,
    face: f64,
    close: Rc<dyn Fn()>,
) -> impl IntoView {
    // Which emoji has the focus, and whether a key has moved it: floem
    // rings the focus only once its own keys have moved it, and these
    // keys are the picker's.
    let current = RwSignal::new(0_usize);
    let keyed = RwSignal::new(false);
    let (ids, picks): (Vec<_>, Vec<_>) = ReactionKind::ALL
        .into_iter()
        .enumerate()
        .map(|(i, kind)| {
            let (id, view) =
                pick(state, backend.clone(), node_id.clone(), kind, keyed, close.clone());
            let view = view
                .into_intermediate()
                .on_event_cont(el::FocusGained, move |_, _| current.set(i));
            (id, view)
        })
        .unzip();
    let first = ids[0];
    let focused = Cell::new(false);
    let card = Stack::horizontal_from_iter(picks)
        // Once the card is styled and placed, and only then, or the emoji
        // cannot take the focus: a view new this frame is not focusable
        // yet. Only the first time, not each time the card moves.
        .on_event_cont(VisualChanged::listener(), move |_, _| {
            if !focused.replace(true) {
                first.request_focus();
            }
        })
        .debug_name("react-picker")
        .style(move |s| {
            let s = s
                .absolute()
                .size(PICKER_WIDTH, PICKER_HEIGHT)
                .items_center()
                .gap(PICK_GAP)
                .padding(PICK_GAP)
                .apply(crate::ui::menu::surface(floem::style::Style::new()));
            let s = if placement.above {
                s.inset_bottom(face + PICKER_OFFSET)
            } else {
                s.inset_top(face + PICKER_OFFSET)
            };
            if placement.rightward {
                s.inset_left(0.0)
            } else {
                s.inset_right(0.0)
            }
        });
    let dismiss = close.clone();
    let backdrop = Empty::new()
        .on_event_stop(el::PointerDown, move |_, _| dismiss())
        .debug_name("react-backdrop")
        .style(|s| s.fixed().inset(0.0));
    // Level with the window's other overlays, not over them as an
    // overlay is by default, so an emoji's tip, added after, is drawn
    // over the card.
    Overlay::new(Stack::new((backdrop, card)).style(|s| s.size_full()))
        .on_event(el::KeyDown, move |cx, key: &KeyboardEvent| {
            let (at, last) = (current.get_untracked(), ids.len() - 1);
            let next = if at == last { 0 } else { at + 1 };
            let previous = if at == 0 { last } else { at - 1 };
            let plain = key.modifiers.is_empty();
            let to = match key.code {
                Code::Escape if plain => {
                    close();
                    return EventPropagation::Stop;
                }
                Code::ArrowRight if plain => next,
                Code::ArrowLeft if plain => previous,
                Code::Tab if plain => next,
                Code::Tab if key.modifiers == Modifiers::SHIFT => previous,
                Code::Home if plain => 0,
                Code::End if plain => last,
                _ => return EventPropagation::Continue,
            };
            // Tab would otherwise go on out of the row as well.
            cx.prevent_default();
            keyed.set(true);
            ids[to].request_focus();
            EventPropagation::Stop
        })
        .style(|s| s.absolute().inset(0.0).z_index(0))
}

/// One of the picker's eight: the emoji alone, with its name in a tip,
/// washed blue while the reader's is among them, and ringed while it has
/// the focus once a key has moved it. One with GitHub takes no click, as
/// its pill takes none, and is drawn faint. The id is the button's, for
/// the focus.
fn pick(
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
    kind: ReactionKind,
    keyed: RwSignal<bool>,
    close: Rc<dyn Fn()>,
) -> (floem::ViewId, impl IntoView) {
    let mine_node = node_id.clone();
    let mine = Memo::new(move |_| {
        state
            .reactions
            .with(|m| m.get(&mine_node).is_some_and(|r| r.viewer_reacted(kind)))
    });
    let busy_node = node_id.clone();
    let busy = Memo::new(move |_| state.is_reacting(&busy_node, kind));
    let button = Container::new(
        Label::derived(move || kind.emoji().to_string()).style(|s| {
            s.font_size(PICK_EMOJI)
                .selectable(false)
                .pointer_events_none()
        }),
    )
    .on_event_stop(el::Click, move |_, _| {
        if !busy.get_untracked() {
            react(state, &backend, &node_id, kind);
            close();
        }
    })
    .debug_name(format!("react-pick:{}", kind.graphql()))
    .style(move |s| {
        let mine = mine.get();
        clickable(s)
            .size(PICK_SIZE, PICK_SIZE)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .apply_if(keyed.get(), |s| {
                s.focus(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            })
            .apply_if(mine, |s| s.background(theme::reacted_bg()))
            .hover(move |s| s.background(if mine { theme::reacted_bg() } else { theme::badge_bg() }))
            .apply_if(busy.get(), |s| s.opacity(0.4_f32))
    });
    let id = button.view_id();
    (id, tip(button, kind.label().to_string()))
}

/// The reactions under an entry's text: a pill per kind anyone left,
/// each with its count, the reader's own washed blue, and the picker at
/// the end. Nothing at all while nobody has reacted; the byline's face
/// is the way in then. A click on a pill toggles the reader's, and a
/// hover says who left it.
fn reactions_row(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
) -> impl IntoView {
    let each_node = node_id.clone();
    let pill_node = node_id.clone();
    let pill_backend = backend.clone();
    let empty_node = node_id.clone();
    let empty = Memo::new(move |_| {
        state
            .reactions
            .with(|m| m.get(&empty_node).is_none_or(|r| r.is_empty()))
    });
    Stack::horizontal((
        dyn_stack(
            move || state.reactions_of(&each_node).groups,
            // The pill is rebuilt on any change to it: it is a glyph and
            // a number, and the row is at most eight of them.
            |g| g.clone(),
            move |g| reaction_pill(app, state, pill_backend.clone(), pill_node.clone(), g),
        )
        .style(|s| s.items_center().gap(6.0).flex_wrap(floem::taffy::FlexWrap::Wrap)),
        react_button(state, backend, node_id, None),
    ))
    .debug_name("reactions")
    .style(move |s| {
        s.items_center()
            .gap(6.0)
            .flex_wrap(floem::taffy::FlexWrap::Wrap)
            .padding_top(theme::REACTIONS_GAP)
            .apply_if(empty.get(), |s| s.hide())
    })
}

/// One reaction: its emoji and how many left it. Blue when the reader is
/// among them. While GitHub is being asked about it, a click is nothing.
fn reaction_pill(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
    group: ReactionGroup,
) -> impl IntoView {
    let who = group.clone();
    let ReactionGroup {
        kind,
        count,
        viewer_reacted: mine,
        ..
    } = group;
    let click_node = node_id.clone();
    let busy = Memo::new(move |_| state.is_reacting(&node_id, kind));
    tip_with(
        Stack::horizontal((
            Label::derived(move || kind.emoji().to_string()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .selectable(false)
                    .pointer_events_none()
            }),
            Label::derived(move || count.to_string()).style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_weight(theme::weight_emphasis())
                    .color(if mine { theme::unread() } else { theme::fg_muted() })
                    .margin_left(5.0)
                    .selectable(false)
                    .pointer_events_none()
            }),
        ))
        .on_event_stop(el::Click, move |_, _| {
            if !busy.get_untracked() {
                react(state, &backend, &click_node, kind);
            }
        })
        .debug_name(format!("reaction:{}", kind.graphql()))
        .style(move |s| {
            clickable(s)
                .items_center()
                .height(theme::REACTION_PILL_HEIGHT)
                .padding_horiz(9.0)
                .border(1.0)
                .border_radius(theme::REACTION_PILL_HEIGHT / 2.0)
                .border_color(if mine { theme::reacted_border() } else { theme::border() })
                .background(if mine { theme::reacted_bg() } else { theme::canvas() })
                .hover(move |s| {
                    s.background(if mine { theme::reacted_bg() } else { theme::badge_bg() })
                        .border_color(if mine { theme::unread() } else { theme::fg_subtle() })
                })
        }),
        move || {
            let viewer = app.viewer.get_untracked().map(|(login, _)| login);
            reacted_tip(&who, viewer.as_deref().filter(|l| !l.is_empty()))
        },
    )
}

/// Who left a reaction, as GitHub's tip says it: "You, octocat, and 3
/// more reacted with heart emoji". The reader is "You", first, by
/// `viewer_reacted`, which a toggle changes before GitHub names them or
/// stops naming them. With nobody named, as in a group an older build
/// kept, it is how many.
fn reacted_tip(group: &ReactionGroup, viewer: Option<&str>) -> String {
    let mut names: Vec<&str> = Vec::new();
    if group.viewer_reacted && viewer.is_some() {
        names.push("You");
    }
    names.extend(
        group
            .reactors
            .iter()
            .map(String::as_str)
            .filter(|login| Some(*login) != viewer),
    );
    names.truncate(group.count as usize);
    let mut parts: Vec<String> = names.into_iter().map(str::to_string).collect();
    let more = group.count as usize - parts.len();
    match (parts.is_empty(), more) {
        (true, 1) => parts.push("1 person".into()),
        (true, n) => parts.push(format!("{n} people")),
        (false, 0) => {}
        (false, n) => parts.push(format!("{n} more")),
    }
    let who = match parts.as_slice() {
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] if !rest.is_empty() => format!("{}, and {last}", rest.join(", ")),
        _ => parts.concat(),
    };
    format!("{who} reacted with {} emoji", group.kind.label().to_lowercase())
}

/// A view with a line of explanation on hover, in the window's overlay so
/// nothing drawn after it can cover it.
pub fn tip<V: IntoView + 'static>(view: V, text: String) -> impl IntoView {
    tip_with(view, move || text.clone())
}

/// The same, with an explanation that follows what it explains: read
/// each time the tip is shown.
pub fn tip_with<V: IntoView + 'static>(
    view: V,
    text: impl Fn() -> String + Clone + 'static,
) -> impl IntoView {
    view.tooltip(move || within_window(tip_body(text.clone()).class(TooltipClass)))
}

/// How far a tip can move left or up to stay in the window: more than
/// any tip is wide or tall.
const TIP_REACH: f64 = 1000.0;

/// A tip placed at a window point, moved left and up as far as it takes
/// to stay inside the window. floem's tooltip and a link's tip set the
/// point as the overlay's `inset_left` and `inset_top` and clamp nothing,
/// so a tip opened near the right edge ran off it.
///
/// The frame starts `TIP_REACH` left of and above the point and runs to
/// the window's right and bottom edges. A spacer as long as the reach
/// puts the tip at the point, and gives way when the tip would overrun
/// the far edge. The frame is invisible to the pointer and takes none of
/// the chrome floem's theme gives a tooltip; that stays on the tip.
pub fn within_window<V: IntoView + 'static>(tip: V) -> Stack {
    let spacer = |horizontal: bool| {
        Empty::new().style(move |s| {
            let s = s.flex_shrink(1.0_f32).pointer_events_none();
            if horizontal {
                s.width(TIP_REACH).min_width(0.0)
            } else {
                s.height(TIP_REACH).min_height(0.0)
            }
        })
    };
    Stack::vertical((
        spacer(false),
        Stack::horizontal((
            spacer(true),
            tip.into_view().style(|s| s.flex_shrink(0.0_f32)),
        ))
        .style(|s| s.flex_shrink(0.0_f32).pointer_events_none()),
    ))
    .debug_name("within window")
    .style(|s| {
        s.margin_left(-TIP_REACH)
            .margin_top(-TIP_REACH)
            .inset_right(0.0)
            .inset_bottom(0.0)
            .padding(0.0)
            .border(0.0)
            .background(Color::TRANSPARENT)
            .apply_box_shadows(Vec::new())
            .pointer_events_none()
    })
}

/// The tooltip itself, for a view that shows one on its own terms.
pub fn tip_body(text: impl Fn() -> String + 'static) -> Label {
    Label::derived(text).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::fg())
            .padding_vert(5.0)
            .padding_horiz(8.0)
            .max_width(320.0)
            .text_wrap()
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .box_shadow_blur(6.0)
            .box_shadow_v_offset(2.0)
            .box_shadow_color(theme::shadow())
    })
}

/// An avatar the caller already has.
pub(crate) fn fixed(url: String) -> Box<dyn Fn() -> String> {
    Box::new(move || url.clone())
}

/// The avatar for a login as it stands when it is drawn: what
/// `avatar_url_for` finds, followed as the payloads it searches land. A
/// GitHub thread's rows are built from the threads event, which can reach
/// the state before the discussion that carries the reviewer's picture.
pub(crate) fn avatar_url_of(state: ReviewState, login: String) -> Box<dyn Fn() -> String> {
    let url = Memo::new(move |_| {
        state.pull_request.track();
        state.issue.track();
        state.discussion.track();
        state.issue_comments.track();
        avatar_url_for(state, &login)
    });
    Box::new(move || url.get())
}

/// The avatar GitHub sent for a login, from whichever payload carries one:
/// the author, a conversation comment, a review comment, a requested
/// reviewer. A review comes with none of its own, and would otherwise
/// show an initial beside the same person's picture.
pub fn avatar_url_for(state: ReviewState, login: &str) -> String {
    state
        .pull_request
        .with_untracked(|p| {
            let p = p.as_ref()?;
            if p.author_login == login {
                return Some(p.author_avatar_url.clone());
            }
            p.requested_reviewers
                .iter()
                .find(|r| r.login == login)
                .map(|r| r.avatar_url.clone())
        })
        .or_else(|| {
            state.issue.with_untracked(|i| {
                i.as_ref()
                    .filter(|i| i.author_login == login)
                    .map(|i| i.author_avatar_url.clone())
            })
        })
        .or_else(|| {
            state.discussion.with_untracked(|d| {
                d.as_ref().and_then(|d| {
                    d.comments
                        .iter()
                        .find(|c| c.user == login)
                        .map(|c| c.user_avatar_url.clone())
                        .or_else(|| {
                            d.review_comments
                                .iter()
                                .find(|c| c.user == login && !c.user_avatar_url.is_empty())
                                .map(|c| c.user_avatar_url.clone())
                        })
                })
            })
        })
        .or_else(|| {
            state.issue_comments.with_untracked(|c| {
                c.iter()
                    .find(|c| c.user == login)
                    .map(|c| c.user_avatar_url.clone())
            })
        })
        .filter(|u| !u.is_empty())
        .unwrap_or_default()
}

/// The composer, with a comment quoted above the draft and, for a line
/// comment, anchored to its line. On Conversation the composer is laid in
/// place and scrolls to itself.
fn ask_about(
    app: AppState,
    state: ReviewState,
    anchor: Option<Anchor>,
    author: &str,
    url: &str,
    body: &str,
) {
    let quote = crate::ask::quote_for(author, url, body);
    // To an agent, whatever was last sent from here: a quoted comment is
    // a question about it, not a reply to it.
    let agent = state
        .agent_last
        .get_untracked()
        .unwrap_or_else(|| threads::defaults(app));
    state.open_composer_with(None, anchor, quote, Destination::agent(agent), None);
}

/// Quote reply at the head of a card's actions: the composer at the foot
/// of Conversation, bound for GitHub, with the text quoted as GitHub's own
/// Quote reply quotes it: what is selected in `scope`, the card's text,
/// or else all of it. GitHub keeps no thread under a comment, a review
/// or the description, so a reply is a new comment at the end, and the
/// quote says what it answers. A draft already bound for GitHub there
/// keeps its text, the quote under it. Absent where there is no pull
/// request or issue to post to, as Reply is.
fn quote_reply_button(
    state: ReviewState,
    hovered: RwSignal<bool>,
    body: String,
    scope: floem::ViewId,
) -> floem::AnyView {
    let quote: Rc<dyn Fn(Option<String>)> =
        Rc::new(move |selected| quote_reply(state, selected.as_deref().unwrap_or(&body)));
    let offered = Memo::new(move |_| {
        state.pull_request.with(Option::is_some) || state.issue.with(Option::is_some)
    });
    dyn_container(
        move || offered.get(),
        move |offered| {
            if !offered {
                return Empty::new().into_any();
            }
            quote_action(hovered, scope, "quote-reply", quote.clone()).into_any()
        },
    )
    .into_any()
}

/// Open the foot composer bound for GitHub with `body` quoted, under
/// whatever the reader had typed there for GitHub.
fn quote_reply(state: ReviewState, body: &str) {
    let quote = crate::ask::quote_reply(body);
    let typed = state
        .composer
        .with_untracked(|c| {
            c.as_ref().is_some_and(|c| {
                c.thread_id.is_none()
                    && c.anchor.is_none()
                    && c.editing.is_none()
                    && c.destination.is_github()
            })
        })
        .then(|| state.composer_draft.get_untracked())
        .filter(|d| !d.trim().is_empty());
    let draft = match typed {
        Some(typed) => format!("{}\n\n{quote}", typed.trim_end()),
        None => quote,
    };
    state.open_composer_with(None, None, draft, Destination::GitHub, None);
}

// ---------------------------------------------------------------------
// GitHub's cards
// ---------------------------------------------------------------------

/// The pull request's or issue's own text, with the reactions on it under.
/// Its strip is its title and the picker; who opened it and when is the
/// summary's Author row.
pub fn description_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    body: String,
    node_id: String,
    editable: bool,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let description = if body.trim().is_empty() {
        Label::derived(|| "No description.".to_string())
            .style(|s| s.font_size(theme::TEXT_BODY).color(theme::fg_subtle()))
            .into_any()
    } else {
        markdown::render_linked(&body, state).into_any()
    };
    let scope = description.id();
    let quote = (!body.trim().is_empty())
        .then(|| quote_reply_button(state, hovered, body.clone(), scope));
    let edit = edit_in_card_action(state, hovered, node_id.clone(), body.clone(), editable);
    let leading = leading_actions(quote, edit, None);
    let react = reactable(&node_id);
    let reactions = reactions_under(app, state, backend.clone(), &react);
    let shown = Stack::vertical((description, reactions))
        .style(|s| s.flex_col().width_full())
        .into_any();
    let shown = edited_in_card(app, state, backend.clone(), node_id, shown);
    track_hover(
        card(
            byline_strip(
                Stack::horizontal((
                    Label::derived(|| "Description".to_string()).style(|s| {
                        s.font_size(theme::TEXT_META)
                            .font_weight(theme::weight_emphasis())
                            .color(theme::fg())
                    }),
                    Empty::new().style(|s| s.flex_grow(1.0_f32)),
                    actions(state, backend, hovered, Some(leading), None, react, String::new()),
                ))
                .style(|s| s.items_center().width_full().min_width(0.0)),
            ),
            Some(shown),
        ),
        hovered,
    )
}

/// One conversation comment, with the reactions on it under.
pub fn comment_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    comment: IssueComment,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let who = Byline {
        login: comment.user.clone(),
        avatar_url: fixed(comment.user_avatar_url.clone()),
        avatar_size: theme::CARD_AVATAR,
        glyph: None,
        verb: "commented".into(),
        when: Some(comment.created_at),
    };
    let (author, url, body) = (
        comment.user.clone(),
        comment.html_url.clone(),
        comment.body.clone(),
    );
    let ask: Rc<dyn Fn()> = Rc::new(move || ask_about(app, state, None, &author, &url, &body));
    let text = markdown::render_linked(&comment.body, state).into_any();
    let quote = quote_reply_button(state, hovered, comment.body.clone(), text.id());
    let deletable = comment.viewer_can_delete && !comment.node_id.is_empty();
    let delete = delete_action(state, hovered, comment.node_id.clone(), move || deletable);
    let edit = edit_in_card_action(
        state,
        hovered,
        comment.node_id.clone(),
        comment.body.clone(),
        comment.viewer_can_update,
    );
    let leading = leading_actions(Some(quote), edit, Some(delete));
    let confirm = delete_comment_confirmation(app, state, backend.clone(), comment.node_id.clone());
    let react = reactable(&comment.node_id);
    let reactions = reactions_under(app, state, backend.clone(), &react);
    let shown = Stack::vertical((text, reactions, confirm))
        .style(|s| s.flex_col().width_full())
        .into_any();
    let body = edited_in_card(app, state, backend.clone(), comment.node_id.clone(), shown);
    track_hover(
        card(
            byline_strip(byline(
                app,
                who,
                actions(state, backend, hovered, Some(leading), Some(ask), react, comment.html_url.clone()),
            )),
            Some(body),
        ),
        hovered,
    )
}

/// Edit under the pointer for what has `node_id` on GitHub, while `may`
/// says GitHub lets the reader edit it: it opens the composer in the
/// card with `body`, its words as written.
fn edit_in_card_action(
    state: ReviewState,
    hovered: RwSignal<bool>,
    node_id: String,
    body: String,
    may: bool,
) -> floem::AnyView {
    let may = may && !node_id.is_empty();
    edit_action(
        hovered,
        move || may,
        Rc::new(move || {
            state.open_composer_with(
                None,
                None,
                body.clone(),
                Destination::GitHub,
                Some(node_id.clone()),
            )
        }),
    )
}

/// What a card shows of `node_id`, which gives way to the composer while
/// the reader edits it, until the edit is sent or put away.
fn edited_in_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
    shown: floem::AnyView,
) -> floem::AnyView {
    // What has no id on GitHub cannot be edited, so it never gives way.
    if node_id.is_empty() {
        return shown;
    }
    let editing = Memo::new(move |_| {
        state.composer.with(|c| {
            c.as_ref()
                .filter(|c| {
                    c.thread_id.is_none() && c.editing.as_deref() == Some(node_id.as_str())
                })
                .map(|c| c.opening)
        })
    });
    let editor = dyn_container(
        move || editing.get(),
        move |opening| match (opening, state.composer.get_untracked()) {
            (Some(_), Some(c)) => {
                threads::composer_view(app, state, backend.clone(), c, ThreadFace::Card)
                    .style(|s| s.width_full())
                    .into_any()
            }
            _ => Empty::new().into_any(),
        },
    )
    .debug_name("comment-editor")
    .style(move |s| s.width_full().apply_if(editing.get().is_none(), |s| s.hide()));
    let shown = Container::new(shown)
        .style(move |s| s.width_full().apply_if(editing.get().is_some(), |s| s.hide()));
    Stack::vertical((shown, editor))
        .style(|s| s.flex_col().width_full())
        .into_any()
}

/// The glyph a verdict takes: a green check, a red page of changes, or a
/// muted comment bubble for anything else.
pub fn verdict_glyph(review: &Review) -> (SidebarIcon, fn() -> Color) {
    match review.state.as_str() {
        "APPROVED" => (SidebarIcon::Check, theme::open),
        "CHANGES_REQUESTED" => (SidebarIcon::ChangesRequested, theme::closed),
        _ => (SidebarIcon::Comment, theme::fg_muted),
    }
}

/// A review: its verdict and summary. Its line comments are threads of
/// their own, each a card where its first comment was left.
pub fn review_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    review: Review,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let login = review.user.clone();
    let summary = Some(review.body.clone()).filter(|b| !b.trim().is_empty());
    let url = review.html_url.clone();
    let who = Byline {
        login: login.clone(),
        avatar_url: fixed(avatar_url_for(state, &login)),
        avatar_size: theme::CARD_AVATAR,
        glyph: Some(verdict_glyph(&review)),
        verb: verdict(&review).to_string(),
        when: review.submitted_at,
    };
    let ask = summary.clone().map(|body| {
        let (author, url) = (login.clone(), url.clone());
        Rc::new(move || ask_about(app, state, None, &author, &url, &body)) as Rc<dyn Fn()>
    });
    let text = summary
        .as_ref()
        .map(|body| markdown::render_linked(body, state).into_any());
    // A summary is quoted and edited where it is; a review with none has
    // nothing to quote or edit here.
    let leading = summary
        .clone()
        .zip(text.as_ref().map(|text| text.id()))
        .map(|(body, scope)| {
            let quote = quote_reply_button(state, hovered, body.clone(), scope);
            let edit = edit_in_card_action(
                state,
                hovered,
                review.node_id.clone(),
                body,
                review.viewer_can_update,
            );
            leading_actions(Some(quote), edit, None)
        });
    // A review with no summary has nothing to react to; GitHub allows it,
    // but there would be nothing to draw the pills under.
    let react = summary.as_ref().and_then(|_| reactable(&review.node_id));

    let body = text.map(|text| {
        let shown = Stack::vertical((
            text,
            reactions_under(app, state, backend.clone(), &react),
        ))
        .style(|s| s.flex_col().width_full())
        .into_any();
        edited_in_card(app, state, backend.clone(), review.node_id.clone(), shown)
    });

    let trailing = actions(state, backend, hovered, leading, ask, react, url);
    track_hover(card(byline_strip(byline(app, who, trailing)), body), hovered)
}

/// The file a thread is on, as the header across it names it: the path
/// in the code face, which opens Files on the file, scrolled to the
/// thread's first line.
fn path_link(state: ReviewState, open_file: OpenFile, anchor: Anchor) -> impl IntoView {
    let path = anchor.path.clone();
    let line = anchor.start_line.min(anchor.line) as usize;
    Label::derived(move || anchor.path.clone())
        .on_event_stop(el::Click, move |_, _| {
            state.show_tab(ItemTab::Code);
            open_file(&path);
            state.jump_to_line.set(Some(line.saturating_sub(1)));
        })
        .debug_name("thread-path")
        .style(|s| {
            clickable(s)
                .font_size(theme::TEXT_BADGE)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .hover(|s| s.color(theme::github()))
        })
}

// ---------------------------------------------------------------------
// Lines of code under a comment or an anchor
// ---------------------------------------------------------------------

/// How a line is tinted: as the diff tints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    Plain,
    Added,
    Removed,
    Modified,
}

impl Tint {
    fn colors(self) -> (Option<Color>, Color) {
        match self {
            Tint::Plain => (None, theme::fg()),
            Tint::Added => (Some(theme::diff_added_bg()), theme::fg()),
            Tint::Modified => (Some(theme::diff_modified_bg()), theme::fg()),
            Tint::Removed => (Some(theme::diff_deleted_bg()), theme::diff_deleted_fg()),
        }
    }
}

/// One line of code as a card shows it: numbered on the side it counts
/// in, tinted, without the diff's `+` or `-`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeLine {
    pub number: Option<usize>,
    pub tint: Tint,
    pub text: String,
}

/// The last `count` lines of a unified hunk, as GitHub sends one under a
/// review comment. The `@@` header gives the numbers and is dropped; a
/// removed line counts on the base side, the rest on the head.
pub fn hunk_tail(hunk: &str, count: usize) -> Vec<CodeLine> {
    let mut lines = Vec::new();
    let (mut old, mut new): (Option<usize>, Option<usize>) = (None, None);
    for line in hunk.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            let start = |prefix: char| {
                header
                    .split_whitespace()
                    .find_map(|p| p.strip_prefix(prefix))
                    .and_then(|p| p.split(',').next())
                    .and_then(|n| n.parse::<usize>().ok())
            };
            old = start('-');
            new = start('+');
            continue;
        }
        let mut chars = line.chars();
        let (tint, text) = match chars.next() {
            Some('+') => (Tint::Added, chars.as_str()),
            Some('-') => (Tint::Removed, chars.as_str()),
            // "\ No newline at end of file" is not a line.
            Some('\\') => continue,
            Some(_) => (Tint::Plain, chars.as_str()),
            None => (Tint::Plain, ""),
        };
        // A removed line counts on the base side, an added one on the
        // head, and a context line on both.
        let number = match tint {
            Tint::Removed => old,
            _ => new,
        };
        if tint != Tint::Added {
            old = old.map(|n| n + 1);
        }
        if tint != Tint::Removed {
            new = new.map(|n| n + 1);
        }
        lines.push(CodeLine {
            number,
            tint,
            text: text.to_string(),
        });
    }
    let skip = lines.len().saturating_sub(count);
    lines.drain(..skip);
    lines
}

/// The lines under an anchor, from the side it names. A head anchor includes
/// removed lines spliced between its range, as the editor shows them. A base
/// anchor reads the retained base text and tints changed/removed lines as
/// removed. At most `max` lines are returned, plus how many more there were.
pub fn anchored_lines(file: &LoadedFile, anchor: &Anchor, max: usize) -> (Vec<CodeLine>, usize) {
    if anchor.side == perch_github::Side::Left {
        return anchored_base_lines(file, anchor, max);
    }
    let first = anchor.start_line.saturating_sub(1) as usize;
    let last = anchor.line.saturating_sub(1) as usize;
    let rows = &file.buffer.rows;
    let from = rows
        .iter()
        .position(|r| r.head_line().is_some_and(|h| h >= first));
    let to = rows
        .iter()
        .rposition(|r| r.head_line().is_some_and(|h| h <= last));
    let (Some(from), Some(to)) = (from, to) else {
        return (Vec::new(), 0);
    };
    if from > to {
        return (Vec::new(), 0);
    }
    let text: Vec<&str> = file.buffer.text.lines().collect();
    let mut lines: Vec<CodeLine> = (from..=to)
        .map(|d| {
            let (number, tint) = match rows[d] {
                Row::Head { head_line, status } => (
                    Some(head_line + 1),
                    match status {
                        LineStatus::Unchanged => Tint::Plain,
                        LineStatus::Added => Tint::Added,
                        LineStatus::Modified => Tint::Modified,
                    },
                ),
                Row::Deleted { base_line } => (Some(base_line + 1), Tint::Removed),
            };
            CodeLine {
                number,
                tint,
                text: text.get(d).copied().unwrap_or_default().to_string(),
            }
        })
        .collect();
    let more = lines.len().saturating_sub(max);
    lines.truncate(max);
    (lines, more)
}

fn anchored_base_lines(file: &LoadedFile, anchor: &Anchor, max: usize) -> (Vec<CodeLine>, usize) {
    let Some(base) = file.base_buffer.as_ref() else {
        return (Vec::new(), 0);
    };
    let text: Vec<&str> = base.text.lines().collect();
    let first = anchor.start_line.min(anchor.line).saturating_sub(1) as usize;
    let last = anchor.start_line.max(anchor.line).saturating_sub(1) as usize;
    if first >= text.len() {
        return (Vec::new(), 0);
    }
    let last = last.min(text.len() - 1);
    let removed: std::collections::HashSet<usize> = file
        .buffer
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Deleted { base_line } => Some(*base_line),
            Row::Head { .. } => None,
        })
        .collect();
    let mut lines: Vec<CodeLine> = (first..=last)
        .map(|base_line| CodeLine {
            number: Some(base_line + 1),
            tint: if removed.contains(&base_line) {
                Tint::Removed
            } else {
                Tint::Plain
            },
            text: text[base_line].to_string(),
        })
        .collect();
    let more = lines.len().saturating_sub(max);
    lines.truncate(max);
    (lines, more)
}

/// The lines, one row each, in the code font. Nothing when there are none.
fn code_lines(lines: Vec<CodeLine>) -> impl IntoView {
    if lines.is_empty() {
        return Empty::new().into_any();
    }
    dyn_stack(
        move || lines.clone().into_iter().enumerate(),
        |(i, _)| *i,
        |(_, line)| code_row(line),
    )
    .style(|s| {
        s.flex_col()
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    })
    .into_any()
}

/// The column a line's `+` or `-` sits in, between its number and its
/// text, as GitHub sets a diff's lines.
const CODE_ROW_MARKER_WIDTH: f64 = 22.0;

pub(crate) fn code_row(line: CodeLine) -> impl IntoView {
    let tint = line.tint;
    let number = line.number.map(|n| n.to_string()).unwrap_or_default();
    let marker = match line.tint {
        Tint::Plain => "",
        Tint::Added | Tint::Modified => "+",
        Tint::Removed => "-",
    };
    let text = line.text;
    Stack::horizontal((
        Label::derived(move || number.clone()).style(|s| {
            s.width(theme::CODE_ROW_NUMBER_WIDTH)
                .padding_right(10.0)
                .text_align(floem::text::Alignment::End)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::TEXT_BADGE)
                .color(theme::gutter_fg())
                .selectable(false)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || marker.to_string()).style(move |s| {
            s.width(CODE_ROW_MARKER_WIDTH)
                .padding_left(8.0)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::TEXT_BADGE)
                .color(tint.colors().1)
                .selectable(false)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || text.clone()).style(move |s| {
            s.font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::TEXT_BADGE)
                .color(tint.colors().1)
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
        }),
    ))
    .style(move |s| {
        let base = s
            .items_center()
            .width_full()
            .height(theme::CODE_ROW_HEIGHT)
            .padding_right(12.0);
        match tint.colors().0 {
            Some(bg) => base.background(bg),
            None => base,
        }
    })
}

/// A last row saying how many lines were left out.
fn more_row(more: usize) -> impl IntoView {
    if more == 0 {
        return Empty::new().into_any();
    }
    Label::derived(move || count_of(more, "more line", "more lines"))
        .style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .color(theme::fg_subtle())
                .padding_left(theme::CODE_ROW_NUMBER_WIDTH + CODE_ROW_MARKER_WIDTH)
                .height(theme::CODE_ROW_HEIGHT)
                .items_center()
                .width_full()
                .selectable(false)
                .border_bottom(1.0)
                .border_color(theme::border())
        })
        .into_any()
}

// ---------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------

/// One line, no card chrome: what was done to the pull request or the
/// issue. The dot sits where the cards' avatars do, and the text where
/// their logins do. `author` is who opened the pull request, for the
/// Opened line, which GitHub's timeline sends without an actor. `link` is
/// the pull request's or the issue's number and page, which the Opened
/// line names after "opened" and opens.
///
/// Everything after the name is the event's own pieces: words, a label
/// in its colours, a commit in the code face, or a reference to open. A
/// commit on the head opens by itself, and a file its subject names opens
/// on Files. The line ends with when it happened.
pub fn event_row(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    event: PullEvent,
    author: Option<String>,
    link: Option<(u64, String)>,
) -> impl IntoView {
    let who = match &event.kind {
        // A commit is signed by a name out of git, which leads to a
        // profile only when GitHub tied it to an account.
        PullEventKind::Committed { login, .. } if !login.is_empty() => {
            Some(Name::Login(login.clone()))
        }
        PullEventKind::Committed { author, .. } if event.actor.is_empty() => {
            (!author.is_empty()).then(|| Name::Plain(author.clone()))
        }
        PullEventKind::Opened if event.actor.is_empty() => author.map(Name::Login),
        _ => (!event.actor.is_empty()).then(|| Name::Login(event.actor.clone())),
    };
    let color: fn() -> Color = match &event.kind {
        PullEventKind::Opened | PullEventKind::Reopened => theme::open,
        PullEventKind::Merged => theme::merged,
        PullEventKind::Closed => theme::closed,
        _ => theme::fg_subtle,
    };
    let at = event.at;
    // A memo, as a byline's time is, so a tick of `now` that leaves the
    // words alone does not lay the line out again.
    let ago = Memo::new(move |_| timefmt::relative(at, app.now.get()));
    let pieces = event_pieces(&event);
    let link = link.filter(|_| event.kind == PullEventKind::Opened);
    // A card's padding and half an avatar puts the dot under the byline
    // avatars; the text then starts where their logins do.
    let dot_left = theme::CARD_PADDING + theme::CARD_AVATAR / 2.0 - theme::EVENT_DOT / 2.0;
    let text_gap = theme::CARD_AVATAR / 2.0 - theme::EVENT_DOT / 2.0 + 9.0;
    Stack::horizontal((
        Empty::new().style(move |s| {
            s.size(theme::EVENT_DOT, theme::EVENT_DOT)
                .border_radius(theme::EVENT_DOT / 2.0)
                .background(color())
                .flex_shrink(0.0_f32)
                .margin_right(text_gap)
        }),
        match who {
            Some(who) => name_label(
                move || who.clone(),
                |s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                        .margin_right(4.0)
                },
            )
            .into_any(),
            None => Empty::new().into_any(),
        },
        dyn_stack(
            move || pieces.clone().into_iter().enumerate(),
            |(i, piece)| format!("{i}:{}", piece.text()),
            move |(_, piece)| event_piece(state, backend.clone(), piece),
        )
        .style(|s| s.items_center().gap(4.0).min_width(0.0)),
        match link {
            Some((number, url)) => Label::derived(move || format!("#{number}"))
                .debug_name("opened link")
                .on_event_stop(el::Click, move |_, _| open_in_browser(&url))
                // Underlined under the pointer, as a name is.
                .style(|s| {
                    underlined_link(
                        s.font_size(theme::TEXT_META)
                            .font_weight(theme::weight_emphasis())
                            .color(theme::fg())
                            .margin_left(4.0)
                            .flex_shrink(0.0_f32),
                    )
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
        Label::derived(move || ago.get()).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .margin_left(4.0)
                .flex_shrink(0.0_f32)
        }),
    ))
    .style(move |s| {
        s.items_center()
            .width_full()
            .height(theme::EVENT_HEIGHT)
            .padding_left(dot_left)
            .padding_right(theme::CARD_PADDING)
    })
}

/// One piece of an event's line.
fn event_piece(state: ReviewState, backend: Rc<Backend>, piece: Piece) -> floem::AnyView {
    if piece.text().is_empty() {
        return Empty::new().into_any();
    }
    match piece {
        Piece::Text(text) => event_words(text).into_any(),
        Piece::Subject(subject) => event_subject(state, subject),
        Piece::Commit(sha) => {
            let short = short_sha(&sha);
            let on_head = sha.clone();
            Label::derived(move || short.clone())
                .debug_name("event commit")
                .on_event_stop(el::Click, move |_, _| open_commit(state, &backend, &sha))
                .style(move |s| {
                    let opens = pull_url(state).is_some() || has_commit(state, &on_head);
                    let s = s
                        .font_family(theme::CODE_FONT_STACK.to_string())
                        .font_size(theme::CODE_FONT_SIZE)
                        .color(theme::fg_muted())
                        .flex_shrink(0.0_f32);
                    if opens {
                        underlined_link(s)
                    } else {
                        s
                    }
                })
                .into_any()
        }
        Piece::Label(label) => crate::review::summary::label_chip(label.name, label.color)
            .style(|s| s.flex_shrink(0.0_f32))
            .into_any(),
        Piece::Sha(sha) => {
            let short = short_sha(&sha);
            Label::derived(move || short.clone())
                .style(|s| {
                    s.font_family(theme::CODE_FONT_STACK.to_string())
                        .font_size(theme::CODE_FONT_SIZE)
                        .color(theme::fg_muted())
                        .flex_shrink(0.0_f32)
                })
                .into_any()
        }
        Piece::Reference(reference) => {
            let url = reference.html_url.clone();
            // A reference GitHub sent without a link is still a line; it
            // just does not promise to open.
            let opens = !url.is_empty();
            let slug = reference.slug();
            let title = reference.title.clone();
            Stack::horizontal((
                Label::derived(move || slug.clone()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg())
                        .font_weight(theme::weight_emphasis())
                        .flex_shrink(0.0_f32)
                }),
                Label::derived(move || title.clone()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_muted())
                        .text_ellipsis()
                        .min_width(0.0)
                }),
            ))
            .on_event_stop(el::Click, move |_, _| follow_link(&url))
            .style(move |s| {
                let s = s.items_center().gap(4.0).min_width(0.0);
                if opens {
                    clickable(s)
                } else {
                    s
                }
            })
            .into_any()
        }
    }
}

/// Words on an event's line, cut short when the line runs out.
fn event_words(text: String) -> Label {
    Label::derived(move || text.clone()).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::fg_muted())
            .text_ellipsis()
            .min_width(0.0)
    })
}

/// Whether the checkout has a commit, so Files can show it by itself.
fn has_commit(state: ReviewState, sha: &str) -> bool {
    state.commits.with_untracked(|commits| commits.iter().any(|c| c.sha == sha))
}

/// The pull request's page, which shows a head commit as one of its own.
/// A bare branch has none, and its commits are all in the checkout.
fn pull_url(state: ReviewState) -> Option<String> {
    state
        .pull_request
        .with(|p| p.as_ref().map(|p| p.html_url.clone()))
        .filter(|url| !url.is_empty())
}

/// Open a head commit: on Files, as the one commit's changes, when the
/// checkout has it, and else under the pull request on GitHub. A
/// checkout that has not fetched a push yet lacks its commits.
fn open_commit(state: ReviewState, backend: &Backend, sha: &str) {
    let item_id = state.item.with_untracked(|i| i.as_ref().map(|i| i.id.clone()));
    match item_id {
        Some(item_id) if has_commit(state, sha) => {
            backend.set_scope(&item_id, perch_git::Scope::Commit { sha: sha.to_string() });
            state.show_tab(ItemTab::Code);
        }
        _ => {
            if let Some(url) = floem::reactive::Effect::untrack(|| pull_url(state)) {
                open_in_browser(&format!("{url}/commits/{sha}"));
            }
        }
    }
}

/// A commit's subject, with each file it names a link to the file on
/// Files. Only a checkout has Files, and a subject in one is drawn again
/// as its listing lands, since what a path names is looked up there.
fn event_subject(state: ReviewState, subject: String) -> floem::AnyView {
    let Some(refs) = crate::review::code_refs::Refs::of(state) else {
        return event_words(subject).into_any();
    };
    let runs = Memo::new(move |_| {
        refs.track_files();
        crate::review::timeline::subject_runs(&subject, |word| {
            refs.file(&crate::review::code_refs::written_place(word)?)
        })
    });
    dyn_container(
        move || runs.get(),
        move |runs| {
            Stack::horizontal_from_iter(runs.into_iter().map(|(text, link)| match link {
                None => event_words(text).into_any(),
                Some(link) => Label::derived(move || text.clone())
                    .debug_name("event file")
                    .on_event_stop(el::Click, move |_, _| refs.go(&link))
                    .style(|s| {
                        underlined_link(
                            s.font_size(theme::TEXT_META)
                                .color(theme::fg())
                                .text_ellipsis()
                                .min_width(0.0),
                        )
                    })
                    .into_any(),
            }))
            .style(|s| s.items_center().min_width(0.0))
            .into_any()
        },
    )
    .style(|s| s.min_width(0.0))
    .into_any()
}

// ---------------------------------------------------------------------
// The thread card
// ---------------------------------------------------------------------

/// A thread on Conversation, as github.com's Conversation draws one: a
/// box headed by the chevron that folds it, the file it is on, Outdated,
/// and Show resolved or Hide resolved on one that is resolved; under the
/// header the lines it is on, its comments, the Follow up bar and the
/// foot with Resolve conversation. A resolved thread is folded to its
/// header, as it arrives on github.com, and Resolve folds one. One of
/// Perch's own is marked Local in its header and has a dashed border, so
/// it is never taken for one of GitHub's.
pub fn thread_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    thread: CommentThread,
) -> impl IntoView {
    let local = thread.origin == perch_github::ThreadOrigin::Local;
    let folded = {
        let id = thread.id.clone();
        Memo::new(move |_| state.is_folded(&id))
    };
    let hunk = thread
        .messages
        .first()
        .filter(|m| m.from_github())
        .and_then(|m| m.diff_hunk.clone());
    let header = thread_header(state, open_file, &thread, folded);
    let body = dyn_container(
        move || folded.get(),
        move |folded| {
            if folded {
                return Empty::new().into_any();
            }
            let thread = thread.clone();
            Stack::vertical((
                match thread.anchor.clone() {
                    Some(anchor) => {
                        anchor_block(state, backend.clone(), anchor, thread.outdated, hunk.clone())
                            .into_any()
                    }
                    None => Empty::new().into_any(),
                },
                threads::notices(app, state, backend.clone(), &thread, ThreadFace::Card),
                threads::messages_view(
                    app,
                    state,
                    backend.clone(),
                    thread.id.clone(),
                    ThreadFace::Card,
                ),
                threads::follow_up(app, state, backend.clone(), thread.id.clone(), ThreadFace::Card),
            ))
            .style(|s| s.flex_col().width_full())
            .into_any()
        },
    )
    .style(|s| s.width_full());
    Stack::vertical((header, body)).style(move |s| {
        let s = s.flex_col().width_full();
        if local {
            perch_surface(s)
        } else {
            github_surface(s)
        }
    })
}

/// The header across a thread on Conversation: the chevron that folds
/// the thread, the file it is on, Outdated, Perch only on one of Perch's
/// own, then Show resolved or Hide resolved on one that is resolved, and
/// the thread's `...` menu. A thread about the item as a whole says so
/// where a file would be. It is on the box's own colour, with a hairline
/// under it while the thread is open: a grey strip would read as Perch's
/// on a GitHub thread.
fn thread_header(
    state: ReviewState,
    open_file: OpenFile,
    thread: &CommentThread,
    folded: Memo<bool>,
) -> impl IntoView {
    let location = match thread.anchor.clone() {
        Some(anchor) => path_link(state, open_file, anchor).into_any(),
        None => Label::derived(move || format!("This {}", threads::subject_word(state)))
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_muted())
                    .selectable(false)
            })
            .into_any(),
    };
    let (chevron_id, toggle_id, open_id) = (thread.id.clone(), thread.id.clone(), thread.id.clone());
    Stack::horizontal((
        threads::chevron_button(
            move || !folded.get(),
            theme::THREAD_HEADER_BUTTON,
            16.0,
            move || state.toggle_fold(&chevron_id),
        )
        .style(|s| s.margin_right(8.0)),
        location,
        threads::outdated_pill(thread.outdated),
        threads::local_chip(!thread.is_github_bound()),
        crate::review::run_view::running_mark(state, thread.id.clone()),
        Empty::new().style(|s| s.flex_grow(1.0_f32).min_width(12.0)),
        threads::resolved_toggle(
            state,
            thread.id.clone(),
            move || folded.get(),
            ThreadFace::Card,
            move || state.toggle_fold(&toggle_id),
        ),
        threads::thread_menu(state, thread, theme::THREAD_MENU, move || {
            if state.is_folded(&open_id) {
                state.toggle_fold(&open_id);
            }
        })
        .style(|s| s.margin_left(4.0)),
    ))
    .debug_name("thread-header")
    .style(move |s| {
        s.items_center()
            .width_full()
            .padding(theme::THREAD_HEADER_PADDING)
            .apply_if(!folded.get(), |s| {
                s.border_bottom(1.0).border_color(theme::border_muted())
            })
    })
}

/// The review finding a thread is the discussion of: from the historical
/// review being read, or else from the item's. `None` for an ordinary
/// thread. Tracked.
pub(crate) fn finding_of(
    app: AppState,
    state: ReviewState,
    thread_id: &str,
) -> Option<crate::review::model::ReviewFindingSummary> {
    let item_id = state
        .item
        .with(|item| item.as_ref().map(|item| item.id.clone()))?;
    match state.historical_findings.get() {
        Some(findings) => findings
            .iter()
            .find(|summary| summary.finding.thread_id == thread_id)
            .cloned(),
        None => app.review_finding_for_thread(&item_id, thread_id),
    }
}

/// Provenance and explicit lifecycle controls for a durable review finding.
/// Ordinary user-created threads have no finding record and render no row.
pub(crate) fn finding_controls(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread_id: String,
    inset: f64,
) -> impl IntoView {
    dyn_container(
        move || {
            let item_id = state
                .item
                .with(|item| item.as_ref().map(|item| item.id.clone()))?;
            let summary = finding_of(app, state, &thread_id)?;
            let original_group = summary
                .finding
                .created_group_id
                .as_deref()
                .and_then(|group_id| {
                    app.review_groups_for(&item_id)
                        .iter()
                        .find(|group| group.group.id == group_id)
                        .cloned()
                });
            // The checkout's head too, so a push redraws whether the
            // code has moved past where the finding was looked at.
            let head = state.item.with(|item| {
                item.as_ref()
                    .and_then(|item| item.checkout.as_ref())
                    .map(|checkout| checkout.head_sha.clone())
            });
            Some((
                summary,
                state.historical_review.with(Option::is_some),
                original_group,
                head,
            ))
        },
        move |summary| match summary {
            Some((summary, historical, original_group, head)) => {
                let finding = &summary.finding;
                // "Claude Opus 5.5 (high)", as the Agents section names a run.
                let reviewer = if summary.attributions.is_empty() {
                    match (&summary.backend, &summary.model) {
                        (Some(backend), Some(model)) => threads::model_name(backend, model),
                        (Some(backend), None) => {
                            crate::review::review_launch::backend_label_from_name(backend).to_string()
                        }
                        _ => "reviewer unavailable".to_string(),
                    }
                } else {
                    summary
                        .attributions
                        .iter()
                        .map(|(backend, model, effort)| threads::agent_name(backend, model, effort))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let seen = if summary.seen_in_reviews.is_empty() {
                    summary
                        .original_review_number
                        .map(|number| format!("found in review #{number}"))
                        .unwrap_or_else(|| "original review unavailable".into())
                } else {
                    format!(
                        "found in {}",
                        summary
                            .seen_in_reviews
                            .iter()
                            .map(|number| format!("#{number}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                // A review's first report of a finding is stored as an
                // observation that it is present, at the review's commit;
                // only a later review seeing it again checks it.
                let rechecked = match summary.original_review_number {
                    Some(original) => summary.seen_in_reviews.iter().any(|n| *n != original),
                    None => summary.seen_in_reviews.len() > 1,
                };
                let fixed = finding.verification == perch_github::FindingVerification::Fixed
                    || finding.disposition == perch_github::FindingDisposition::VerifiedFixed;
                let checked = finding.last_verified_head_sha.clone();
                let checked_word = if fixed {
                    "\u{b7} fixed at"
                } else if rechecked {
                    "\u{b7} checked at"
                } else {
                    "at"
                };
                // The checkout has moved past where the finding was last
                // looked at, so what it says may no longer hold.
                let last_looked = finding
                    .last_checked_head_sha
                    .clone()
                    .or_else(|| finding.last_verified_head_sha.clone());
                let moved = !fixed
                    && finding.disposition == perch_github::FindingDisposition::Open
                    && head
                        .as_deref()
                        .zip(last_looked.as_deref())
                        .is_some_and(|(head, sha)| head != sha);
                let status = finding_status(finding, rechecked);
                let reason = (finding.disposition == perch_github::FindingDisposition::Dismissed)
                    .then(|| finding.dismissal_reason.clone())
                    .flatten();
                let (severity_ink, severity_fill) = severity_inks(summary.severity);
                let severity = finding_severity_label(summary.severity);
                let finding_id = finding.id.clone();
                let dismiss_backend = backend.clone();
                let dismiss = if finding.disposition == perch_github::FindingDisposition::Open
                    && finding.verification != perch_github::FindingVerification::Fixed
                    && finding.duplicate_of.is_none()
                    && !historical
                {
                    thread_action("Dismiss", move || {
                        dismiss_backend
                            .dismiss_review_finding(&finding_id, "Dismissed by the user in Perch");
                    })
                    .into_any()
                } else {
                    absent()
                };

                let finding_id = finding.id.clone();
                let reopen_backend = backend.clone();
                let reopen = if finding.disposition == perch_github::FindingDisposition::Dismissed
                    && !historical
                {
                    thread_action("Reopen", move || {
                        reopen_backend.reopen_review_finding(&finding_id);
                    })
                    .into_any()
                } else {
                    absent()
                };

                let finding_id = finding.id.clone();
                let duplicate_backend = backend.clone();
                let link = match (
                    finding.duplicate_of.as_ref(),
                    finding.possible_duplicate_of.as_ref(),
                ) {
                    (None, Some(candidate)) if !historical => {
                        let candidate = candidate.clone();
                        thread_action("Link duplicate", move || {
                            duplicate_backend.link_review_duplicate(&finding_id, &candidate);
                        })
                        .into_any()
                    }
                    _ => absent(),
                };

                let finding_id = finding.id.clone();
                let unlink_backend = backend.clone();
                let unlink = if finding.duplicate_of.is_some() && !historical {
                    thread_action("Unlink", move || {
                        unlink_backend.unlink_review_duplicate(&finding_id);
                    })
                    .into_any()
                } else {
                    absent()
                };

                let original_backend = backend.clone();
                let view_original = match original_group {
                    Some(group) if !historical => {
                        let group_id = group.group.id.clone();
                        let item_id = group.group.item_id.clone();
                        quiet_action("View original", move || {
                            let request_id =
                                original_backend.open_review_snapshot(&item_id, &group_id);
                            app.begin_historical_review(state, group.clone(), request_id);
                        })
                        .into_any()
                    }
                    _ => absent(),
                };

                let sha_backend = backend.clone();
                let provenance = Stack::horizontal((
                    sidebar_glyph(SidebarIcon::Agents, theme::fg_subtle)
                        .style(|s| s.size(12.0, 12.0).flex_shrink(0.0_f32)),
                    Label::derived(move || reviewer.clone())
                        .style(|s| finding_meta(s).color(theme::fg_muted())),
                    Label::derived(move || format!("\u{b7} {seen}")).style(finding_meta),
                    match checked {
                        Some(sha) => Stack::horizontal((
                            Label::derived(move || checked_word.to_string()).style(finding_meta),
                            {
                                let open = sha.clone();
                                Label::derived(move || short_sha(&sha).to_string())
                                    .debug_name("finding-checked-sha")
                                    .on_event_stop(el::Click, move |_, _| {
                                        open_commit(state, &sha_backend, &open)
                                    })
                                    .style(|s| {
                                        underlined_link(
                                            finding_meta(s)
                                                .font_family(theme::CODE_FONT_STACK.to_string())
                                                .color(theme::link()),
                                        )
                                    })
                            },
                        ))
                        .style(|s| s.items_center().gap(4.0))
                        .into_any(),
                        None => Label::derived(|| "\u{b7} not yet checked".to_string())
                            .style(finding_meta)
                            .into_any(),
                    },
                    Label::derived(|| "\u{b7} code changed since".to_string())
                        .debug_name("finding-moved")
                        .style(move |s| {
                            finding_meta(s)
                                .color(theme::notice_fg())
                                .apply_if(!moved, |s| s.hide())
                        }),
                ))
                .style(|s| s.items_center().gap(4.0).flex_wrap(floem::style::FlexWrap::Wrap));
                Stack::vertical((
                    Stack::horizontal((
                        crate::ui::widgets::badge(
                            move || severity.to_string(),
                            severity_ink,
                            severity_fill,
                            None,
                        ),
                        Label::derived(|| "Review finding".to_string()).style(|s| {
                            s.font_size(theme::TEXT_META)
                                .font_weight(theme::weight_emphasis())
                                .color(theme::fg())
                                .flex_shrink(0.0_f32)
                                .selectable(false)
                        }),
                        crate::ui::widgets::badge(move || status.0.clone(), status.1, status.2, None)
                            .debug_name("finding-status"),
                        match reason {
                            Some(reason) => Label::derived(move || reason.clone())
                                .style(|s| finding_meta(s).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32))
                                .into_any(),
                            None => absent(),
                        },
                        Empty::new().style(|s| s.flex_grow(1.0_f32).min_width(8.0)),
                        view_original,
                        dismiss,
                        reopen,
                        link,
                        unlink,
                    ))
                    .style(|s| s.items_center().gap(6.0).width_full().min_height(24.0)),
                    provenance,
                ))
                .debug_name("finding-band")
                // Perch's own note on a thread, so on Perch's grey, from
                // one side of the box to the other as the reader's asks
                // are.
                .style(move |s| {
                    s.flex_col()
                        .width_full()
                        .gap(6.0)
                        .padding_vert(10.0)
                        .padding_horiz(inset)
                        .background(theme::perch_bg())
                        .border_bottom(1.0)
                        .border_color(theme::border_muted())
                })
                .into_any()
            }
            None => Empty::new().into_any(),
        },
    )
    .style(|s| s.width_full())
}

/// A chip's ink or fill, read as it is drawn so it follows the appearance.
pub(crate) type ChipInk = fn() -> Color;

/// What a finding's status chip says, and its ink and fill: fixed in
/// green, a duplicate in GitHub's blue, disputed in the notice's yellow,
/// and the rest grey. Present is Still present only once a review after
/// the one that found it has `rechecked` it; until then it is Reported.
fn finding_status(finding: &perch_github::Finding, rechecked: bool) -> (String, ChipInk, ChipInk) {
    use perch_github::{FindingDisposition, FindingVerification};
    if let Some(canonical) = finding.duplicate_of.as_deref() {
        let text = format!("Duplicate of {}", short_finding(canonical));
        return (text, theme::news_fg, theme::news_bg);
    }
    if let Some(candidate) = finding.possible_duplicate_of.as_deref() {
        let text = format!("Possible duplicate of {}", short_finding(candidate));
        return (text, theme::news_fg, theme::news_bg);
    }
    if finding.disposition == FindingDisposition::Dismissed {
        return ("Dismissed".into(), theme::fg_muted, theme::badge_bg);
    }
    if finding.verification == FindingVerification::Fixed
        || finding.disposition == FindingDisposition::VerifiedFixed
    {
        return ("\u{2713} Verified fixed".into(), theme::open, theme::diff_added_bg);
    }
    match finding.verification {
        FindingVerification::Present if rechecked => {
            ("Still present".into(), theme::fg_muted, theme::badge_bg)
        }
        FindingVerification::Present => ("Reported".into(), theme::fg_muted, theme::badge_bg),
        FindingVerification::Disputed => ("Disputed".into(), theme::notice_fg, theme::notice_bg),
        FindingVerification::NotVerified => {
            ("Couldn't verify".into(), theme::fg_muted, theme::badge_bg)
        }
        _ => ("Not yet checked".into(), theme::fg_muted, theme::badge_bg),
    }
}

/// A severity chip's ink and fill: High and Critical in red, Medium in
/// the notice's yellow, the rest grey.
pub(crate) fn severity_inks(severity: perch_github::FindingSeverity) -> (ChipInk, ChipInk) {
    use perch_github::FindingSeverity;
    match severity {
        FindingSeverity::Critical | FindingSeverity::High => (theme::closed, theme::diff_deleted_bg),
        FindingSeverity::Medium => (theme::notice_fg, theme::notice_bg),
        _ => (theme::fg_muted, theme::badge_bg),
    }
}

/// What stands in for an action a finding does not offer: hidden, so it
/// takes no share of the row's gap, which would push the rest in from
/// the band's edge.
fn absent() -> floem::AnyView {
    Empty::new().style(|s| s.hide()).into_any()
}

/// The finding band's quiet second line.
fn finding_meta(s: floem::style::Style) -> floem::style::Style {
    s.font_size(theme::TEXT_BADGE)
        .color(theme::fg_subtle())
        .flex_shrink(0.0_f32)
        .selectable(false)
}

/// An action said in a word, not a button: the one a finding offers
/// besides what is done to it.
fn quiet_action(label: &'static str, action: impl Fn() + 'static) -> Label {
    Label::derived(move || label.to_string())
        .on_event_stop(el::Click, move |_, _| action())
        .style(|s| {
            clickable(s)
                .font_size(theme::TEXT_BADGE)
                .color(theme::fg_muted())
                .padding_horiz(6.0)
                .height(24.0)
                .items_center()
                .border_radius(theme::RADIUS)
                .selectable(false)
                .hover(|s| s.color(theme::fg()).background(theme::canvas_hover()))
        })
}

/// A small bordered button for what can be done to a thread: a finding's
/// Dismiss and Reopen, and any other's Resolve.
pub(crate) fn thread_action(label: &'static str, action: impl Fn() + 'static) -> Label {
    Label::derived(move || label.to_string())
        .on_event_stop(el::Click, move |_, _| action())
        .style(|s| {
            clickable(s)
                .height(24.0)
                .padding_horiz(8.0)
                .items_center()
                .font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg_muted())
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .selectable(false)
        })
}

pub(crate) fn finding_severity_label(severity: perch_github::FindingSeverity) -> &'static str {
    match severity {
        perch_github::FindingSeverity::Unknown => "Finding",
        perch_github::FindingSeverity::Note => "Note",
        perch_github::FindingSeverity::Low => "Low",
        perch_github::FindingSeverity::Medium => "Medium",
        perch_github::FindingSeverity::High => "High",
        perch_github::FindingSeverity::Critical => "Critical",
    }
}

fn short_finding(value: &str) -> String {
    format!("finding {}", &value[..value.len().min(8)])
}

/// The lines a thread is on, under its header. They come from the loaded
/// file, which is asked for when nothing has loaded it yet; a load fills
/// `files` without opening a tab. A thread from GitHub brings the hunk
/// its comment was left on, `hunk`, which stands in for the file when
/// the thread is outdated or the checkout is not at the pull request's
/// head, since GitHub's line numbers are the head's: a root ahead of the
/// pull request, or a checkout left stale, would put the comment's number
/// on another line. The window is then the hunk's: the commented line
/// and the few above it.
fn anchor_block(
    state: ReviewState,
    backend: Rc<Backend>,
    anchor: Anchor,
    outdated: bool,
    hunk: Option<String>,
) -> impl IntoView {
    let path = anchor.path.clone();
    let loaded = Memo::new(move |_| state.files.with(|m| m.contains_key(&path)));
    let hunk = hunk.map(|h| hunk_tail(&h, theme::HUNK_LINES)).filter(|h| !h.is_empty());
    let at_head = Memo::new(move |_| {
        let checkout = state.item.with(|i| {
            i.as_ref()
                .and_then(|i| i.checkout.as_ref())
                .map(|c| c.head_sha.clone())
        });
        let pr = state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.head_sha.clone()));
        checkout.is_some() && checkout == pr && state.stale_head.with(Option::is_none)
    });
    // The file's lines are wanted when there is no hunk to fall back on,
    // or when the file is where the hunk's numbers count.
    let has_hunk = hunk.is_some();
    let from_file = move || !has_hunk || (!outdated && at_head.get());
    if from_file() && state.begin_load(&anchor.path) {
        backend.open_file(&anchor.path);
    }
    // A comment from GitHub on one line is shown with the few above it,
    // as its hunk would show them, so the line is not alone.
    let lines_anchor = if has_hunk && anchor.start_line == anchor.line {
        Anchor {
            start_line: anchor.line.saturating_sub(theme::HUNK_LINES as u32 - 1).max(1),
            ..anchor.clone()
        }
    } else {
        anchor.clone()
    };
    dyn_container(
        move || (loaded.get(), from_file()),
        move |(loaded, from_file)| {
            let file = (loaded && from_file)
                .then(|| state.file(&lines_anchor.path))
                .flatten()
                .filter(|f| !has_hunk || f.scope == perch_git::Scope::Branch);
            match (file, &hunk) {
                (Some(file), _) => {
                    let (lines, more) = anchored_lines(&file, &lines_anchor, theme::ANCHOR_LINES);
                    Stack::vertical((code_lines(lines), more_row(more)))
                        .style(|s| s.flex_col().width_full())
                        .into_any()
                }
                (None, Some(hunk)) => code_lines(hunk.clone()).into_any(),
                (None, None) => Empty::new().into_any(),
            }
        },
    )
    .debug_name("thread-lines")
    .style(|s| s.width_full())
}

// ---------------------------------------------------------------------
// The composer's place
// ---------------------------------------------------------------------

/// A row that opens the composer when nothing is in it: the box the
/// question will go in, with its placeholder, before it is a box.
pub fn ask_box(app: AppState, state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Thread, theme::fg_subtle)
            .style(|s| s.size(13.0, 13.0).margin_right(8.0)),
        Label::derived(move || format!("Ask about this {}…", threads::subject_word(state))).style(
            |s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::fg_subtle())
                    .selectable(false)
            },
        ),
    ))
    .on_event_stop(el::Click, move |_, _| {
        state.open_composer(None, None, String::new(), threads::defaults(app));
    })
    .style(|s| {
        s.items_center()
            .width_full()
            .height(theme::ASK_BOX_HEIGHT)
            .padding_horiz(18.0)
            .background(theme::perch_bg())
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(14.0)
            .hover(|s| {
                s.background(theme::canvas_hover())
                    .border_color(theme::border())
                    .cursor(CursorStyle::Text)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use floem::action::add_overlay;
    use floem::headless::{HeadlessHarness, TestRoot};

    use crate::review::header::tests::{named, rect};

    fn hearts(count: u32, mine: bool, reactors: &[&str]) -> ReactionGroup {
        ReactionGroup {
            kind: ReactionKind::Heart,
            count,
            viewer_reacted: mine,
            reactors: reactors.iter().map(|r| r.to_string()).collect(),
        }
    }

    /// A pill's tip names who left it as GitHub's does, the reader as
    /// You, first, whether or not GitHub has caught up with a toggle.
    /// A finding one review reported is Reported; Still present needs a
    /// later review to have seen it again. Fixed and dismissed say so
    /// whatever the reviews were.
    #[test]
    fn a_finding_is_still_present_only_once_a_later_review_saw_it() {
        use perch_github::{FindingDisposition, FindingVerification};
        let at: jiff::Timestamp = "2026-09-18T09:59:00Z".parse().unwrap();
        let finding = |disposition, verification| perch_github::Finding {
            id: "f-1".into(),
            item_id: "item-1".into(),
            stable_key: "k".into(),
            thread_id: "t".into(),
            created_group_id: None,
            disposition,
            verification,
            dismissal_reason: None,
            possible_duplicate_of: None,
            duplicate_of: None,
            last_verified_snapshot_id: None,
            last_verified_head_sha: Some("d1dc594".into()),
            last_checked_snapshot_id: None,
            last_checked_head_sha: Some("d1dc594".into()),
            created_at: at,
            updated_at: at,
        };
        let present = finding(FindingDisposition::Open, FindingVerification::Present);
        assert_eq!(finding_status(&present, false).0, "Reported");
        assert_eq!(finding_status(&present, true).0, "Still present");
        let fixed = finding(FindingDisposition::VerifiedFixed, FindingVerification::Fixed);
        assert_eq!(finding_status(&fixed, false).0, "\u{2713} Verified fixed");
        let dismissed = finding(FindingDisposition::Dismissed, FindingVerification::Present);
        assert_eq!(finding_status(&dismissed, true).0, "Dismissed");
    }

    #[test]
    fn a_reactions_tip_names_who_left_it() {
        let me = Some("me");
        let tip = |g: ReactionGroup| reacted_tip(&g, me);
        assert_eq!(tip(hearts(1, false, &["octocat"])), "octocat reacted with heart emoji");
        assert_eq!(tip(hearts(2, false, &["a", "b"])), "a and b reacted with heart emoji");
        assert_eq!(tip(hearts(3, false, &["a", "b", "c"])), "a, b, and c reacted with heart emoji");
        assert_eq!(tip(hearts(12, false, &["a", "b"])), "a, b, and 10 more reacted with heart emoji");
        assert_eq!(tip(hearts(5, false, &["a"])), "a and 4 more reacted with heart emoji");
        assert_eq!(tip(hearts(2, true, &["a", "me"])), "You and a reacted with heart emoji");
        // Just added, and GitHub not yet asked: You, counted in.
        assert_eq!(tip(hearts(2, true, &["a"])), "You and a reacted with heart emoji");
        // Just taken back: the reader's name goes before GitHub's answer.
        assert_eq!(tip(hearts(1, false, &["me", "a"])), "a reacted with heart emoji");
        // Nobody named, as an older build kept it.
        assert_eq!(tip(hearts(1, false, &[])), "1 person reacted with heart emoji");
        assert_eq!(tip(hearts(3, false, &[])), "3 people reacted with heart emoji");
        assert_eq!(tip(hearts(1, true, &[])), "You reacted with heart emoji");
        // The reader not known yet: GitHub's names as they are.
        assert_eq!(
            reacted_tip(&hearts(2, true, &["me", "a"]), None),
            "me and a reacted with heart emoji"
        );
        assert_eq!(
            reacted_tip(&ReactionGroup { kind: ReactionKind::ThumbsUp, ..hearts(1, false, &["a"]) }, me),
            "a reacted with thumbs up emoji"
        );
    }

    /// Where a tip opened at `at` in a 400 by 300 window lands, and the
    /// view under a point beside it. The frame is given the class and
    /// place floem's tooltip gives it.
    fn tip_at(at: (f64, f64)) -> (Rect, Option<floem::ViewId>) {
        let root = TestRoot::new();
        let page = Empty::new().debug_name("page").style(|s| s.size_full());
        let mut harness = HeadlessHarness::new_with_size(root, page, 400.0, 300.0);
        add_overlay(
            within_window(tip_body(|| "Agents".to_string()).debug_name("tip"))
                .class(TooltipClass)
                .style(move |s| s.inset_left(at.0).inset_top(at.1)),
        );
        for _ in 0..2 {
            harness.rebuild();
            harness.paint();
        }
        let page = named(harness.root_id(), "page").unwrap();
        let hit = harness
            .element_id_at(5.0, 5.0)
            .map(|e| e.owning_id())
            .filter(|id| *id == page);
        (rect(&harness, "tip"), hit)
    }

    #[test]
    fn a_tip_stays_inside_the_window() {
        let (tip, hit) = tip_at((20.0, 20.0));
        assert_eq!(
            (tip.x0, tip.y0),
            (20.0, 20.0),
            "room to spare: at the point"
        );
        assert!(hit.is_some(), "the frame lets the pointer through");

        let (tip, _) = tip_at((390.0, 20.0));
        assert_eq!(
            (tip.x1.round(), tip.y0),
            (400.0, 20.0),
            "moved left to the right edge"
        );

        let (tip, _) = tip_at((20.0, 295.0));
        assert_eq!(
            (tip.x0, tip.y1.round()),
            (20.0, 300.0),
            "moved up to the bottom edge"
        );
    }

    #[test]
    fn a_hunks_last_lines_are_numbered_on_the_side_they_count_in() {
        let hunk = "@@ -40,3 +40,4 @@ fn write()\n let a = 1;\n-let b = 2;\n+let b = 3;\n+let c = 4;\n let d = 5;\n\\ No newline at end of file";
        let lines = hunk_tail(hunk, 4);
        assert_eq!(
            lines,
            vec![
                CodeLine {
                    number: Some(41),
                    tint: Tint::Removed,
                    text: "let b = 2;".into()
                },
                CodeLine {
                    number: Some(41),
                    tint: Tint::Added,
                    text: "let b = 3;".into()
                },
                CodeLine {
                    number: Some(42),
                    tint: Tint::Added,
                    text: "let c = 4;".into()
                },
                CodeLine {
                    number: Some(43),
                    tint: Tint::Plain,
                    text: "let d = 5;".into()
                },
            ]
        );
        // The whole hunk when it is short, and nothing from nothing.
        assert_eq!(hunk_tail(hunk, 10).len(), 5);
        assert_eq!(
            hunk_tail(hunk, 10)[0].number,
            Some(40),
            "a context line counts on the head side"
        );
        assert!(hunk_tail("", 4).is_empty());
        assert!(hunk_tail("@@ -1 +1 @@", 4).is_empty());
    }

    #[test]
    fn a_hunk_without_a_header_has_lines_but_no_numbers() {
        let lines = hunk_tail(" a\n+b", 4);
        assert_eq!(
            lines.iter().map(|l| l.number).collect::<Vec<_>>(),
            [None, None]
        );
        assert_eq!(lines[1].tint, Tint::Added);
    }

    #[test]
    fn anchored_lines_come_from_the_buffer_with_the_removed_lines_between() {
        let file = crate::backend::build_loaded_file(
            "src/x.rs".into(),
            "a\nb2\nc\nd\n".into(),
            "a\nb\nc\nd\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            String::new(),
        );
        let anchor = |start, line| Anchor {
            path: "src/x.rs".into(),
            start_line: start,
            line,
            side: perch_github::Side::Right,
            commit_sha: "h".into(),
        };
        let (lines, more) = anchored_lines(&file, &anchor(1, 3), 8);
        assert_eq!(more, 0);
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.number, l.tint, l.text.as_str()))
                .collect::<Vec<_>>(),
            [
                (Some(1), Tint::Plain, "a"),
                (Some(2), Tint::Removed, "b"),
                (Some(2), Tint::Modified, "b2"),
                (Some(3), Tint::Plain, "c")
            ]
        );
        let (lines, more) = anchored_lines(&file, &anchor(1, 4), 2);
        assert_eq!(
            (lines.len(), more),
            (2, 3),
            "cut to the cap, and the rest counted"
        );
        assert!(
            anchored_lines(&file, &anchor(40, 50), 8).0.is_empty(),
            "past the end is nothing"
        );

        // The same numeric line on the base side is the old b alone, not
        // the old/new pair that occupies head line 2 in the unified view.
        let mut base_anchor = anchor(2, 2);
        base_anchor.side = perch_github::Side::Left;
        let (lines, more) = anchored_lines(&file, &base_anchor, 8);
        assert_eq!(more, 0);
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.number, line.tint, line.text.as_str()))
                .collect::<Vec<_>>(),
            [(Some(2), Tint::Removed, "b")]
        );
    }

    #[test]
    fn the_key_follows_what_github_can_edit_and_not_a_threads_messages() {
        use crate::review::timeline::tests::at;
        let comment = |edited: Option<&str>| {
            Card::Comment(IssueComment {
                node_id: String::new(),
                reactions: Default::default(),
                id: 7,
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "q".into(),
                created_at: at("2026-09-15T09:14:00Z"),
                updated_at: edited.map(at),
                html_url: String::new(),
                viewer_can_delete: false,
                viewer_can_update: false,
            })
        };
        assert_ne!(
            key(&comment(None)),
            key(&comment(Some("2026-09-16T09:14:00Z")))
        );
        let event = |kind| {
            Card::Event(PullEvent::new(kind, at("2026-09-15T09:14:00Z"), "rohan"))
        };
        assert_ne!(
            key(&event(PullEventKind::Opened)),
            key(&event(PullEventKind::Closed))
        );
    }

    /// The picker opens the way its face prefers while the window has room
    /// there, and turns on an axis only when the other way has room.
    #[test]
    fn the_picker_turns_only_where_it_would_leave_the_window() {
        let window = Size::new(1000.0, 800.0);
        let face = |x: f64, y: f64| Rect::new(x, y, x + 26.0, y + 26.0);
        let row = Placement::preferred(false);
        let byline = Placement::preferred(true);
        let at = |above, rightward| Placement { above, rightward };

        assert_eq!(row.fitted(face(100.0, 400.0), window), at(true, true));
        assert_eq!(byline.fitted(face(700.0, 400.0), window), at(false, false));
        // Too near the top for the row's, too near the bottom for the
        // byline's.
        assert_eq!(row.fitted(face(100.0, 20.0), window), at(false, true));
        assert_eq!(byline.fitted(face(700.0, 760.0), window), at(true, false));
        // Too near the right for the row's, too near the left for the
        // byline's.
        assert_eq!(row.fitted(face(900.0, 400.0), window), at(true, false));
        assert_eq!(byline.fitted(face(100.0, 400.0), window), at(false, true));
        // A window with room on neither side keeps the face's own way.
        let small = Size::new(200.0, 60.0);
        assert_eq!(row.fitted(face(100.0, 20.0), small), row);
        assert_eq!(byline.fitted(face(100.0, 20.0), small), byline);
    }
}
