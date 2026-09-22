//! The entries on Conversation: the description, each comment and review
//! from GitHub, a line for each event, and each thread, the reader's own
//! and GitHub's review threads alike. Conversation is a flat reading
//! surface; its hierarchy comes from type, whitespace and hairlines rather
//! than a stack of framed cards.
//!
//! A local thread and a GitHub thread must never look like one. Nothing
//! local reaches GitHub, so a local thread keeps an inset rule and a
//! Local chip while GitHub entries remain directly on white. The same
//! distinction holds in the code, where a GitHub thread's card under its
//! line is white and solid, against the grey and dashed card of the
//! reader's own.
//!
//! What a GitHub entry does carry of the reader's is its reactions: the
//! pills under its text, and the face in its byline that opens the
//! picker. A click toggles the pill where it is and asks GitHub after.

use std::rc::Rc;

use floem::peniko::Color;
use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, Stack, TooltipExt,
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
use crate::review::timeline::{event_name, event_pieces, short_sha, verdict, Card, Piece};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{
    avatar, clickable, icon_button, name_label, open_in_browser, sidebar_glyph, Name,
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
        Card::Description { body, at, .. } => {
            format!("description:{:?}:{:x}", at, body_hash(body))
        }
        Card::Comment(c) => format!("comment:{}:{:?}", c.id, c.updated_at),
        Card::Review { review } => {
            format!("review:{}:{:?}:{:x}", review.id, review.submitted_at, body_hash(&review.body))
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

/// One flat Conversation entry: a compact byline, then the text, with a
/// single hairline separating it from the next entry.
pub fn card(header: impl IntoView + 'static, body: floem::AnyView) -> impl IntoView {
    Stack::vertical((
        header,
        Container::new(body).style(|s| {
            s.width_full()
                .padding_top(10.0)
                .padding_bottom(theme::CONVERSATION_ENTRY_PADDING)
        }),
    ))
    .style(|s| {
        s.width_full()
            .flex_col()
            .background(theme::CANVAS)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

/// The compact metadata row at the top of a flat entry.
fn byline_strip(content: impl IntoView + 'static) -> impl IntoView {
    Container::new(content).style(|s| {
        s.items_center()
            .width_full()
            .height(theme::BYLINE_HEIGHT)
            .background(theme::CANVAS)
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
    pub(crate) glyph: Option<(SidebarIcon, Color)>,
    pub(crate) verb: String,
    pub(crate) when: Option<jiff::Timestamp>,
}

/// Avatar, login, what they did and when, and whatever the caller puts at
/// the right: the permalink and Ask about this.
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
                    .color(theme::FG)
            },
        ),
        Label::derived(move || match when {
            Some(at) if verb.is_empty() => timefmt::relative(at, app.now.get()),
            Some(at) => format!("{verb} {}", timefmt::relative(at, app.now.get())),
            None => verb.clone(),
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_MUTED)
                .margin_left(6.0)
                .text_ellipsis()
                .min_width(0.0)
        }),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        trailing,
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// The right end of a GitHub byline: whatever the caller puts first, a
/// Reply on a review comment; Ask about this and the reaction picker
/// while the card is hovered, and the permalink always. `react` is the
/// subject's node id; an entry an older build kept without one has no
/// picker, since GitHub could not be told what it was for.
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
            Some(ask) => text_action(hovered, SidebarIcon::Thread, "Ask about this", "ask-about", ask)
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
            icon_button(
                || SidebarIcon::Link,
                || theme::FG_SUBTLE,
                move || open_in_browser(&url),
            )
            .debug_name("github-link")
            .style(|s| s.size(24.0, 24.0).margin_right(-4.0))
            .into_any()
        },
    ))
    .style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32))
    .into_any()
}

/// A glyph and a word, "Ask about this" or "Reply", drawn only while the
/// card is under the pointer. Hidden rather than absent, so the byline
/// keeps its shape.
pub(crate) fn text_action(
    hovered: RwSignal<bool>,
    icon: SidebarIcon,
    label: &'static str,
    name: &'static str,
    action: Rc<dyn Fn()>,
) -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(icon, theme::FG_MUTED).style(|s| s.size(12.0, 12.0).margin_right(5.0)),
        Label::derived(move || label.to_string()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::FG_MUTED)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| action())
    .debug_name(name)
    .style(move |s| {
        clickable(s)
            .items_center()
            .height(24.0)
            .padding_horiz(8.0)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::BADGE_BG))
            .apply_if(!hovered.get(), |s| s.hide())
    })
}

/// Follow the pointer in and out of a card, for Ask about this.
pub(crate) fn track_hover<V: IntoView + 'static>(view: V, hovered: RwSignal<bool>) -> impl IntoView {
    view.into_intermediate()
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
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
    state: ReviewState,
    backend: Rc<Backend>,
    react: &Option<String>,
) -> floem::AnyView {
    match react {
        Some(node_id) => reactions_row(state, backend, node_id.clone()).into_any(),
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

/// The picker: GitHub's eight, each checked when the reader has left it,
/// so picking one again takes it back. Built on each open, so it shows
/// the reactions as they stand then.
fn reaction_menu(state: ReviewState, backend: Rc<Backend>, node_id: String) -> Menu {
    let reactions = state
        .reactions
        .with_untracked(|m| m.get(&node_id).cloned())
        .unwrap_or_default();
    let mut menu = Menu::new();
    for kind in ReactionKind::ALL {
        let mine = reactions.viewer_reacted(kind);
        let busy = state.reacting.with_untracked(|r| r.contains(&(node_id.clone(), kind)));
        let (node_id, backend) = (node_id.clone(), backend.clone());
        menu = menu.item(format!("{}  {}", kind.emoji(), kind.label()), move |item| {
            item.checked(mine)
                .enabled(!busy)
                .action(move || react(state, &backend, &node_id, kind))
        });
    }
    menu
}

/// A face that opens the picker. In a byline it is drawn only while the
/// card is under the pointer, as Ask about this is, and hidden rather
/// than absent so the byline keeps its shape; at the end of a row of
/// pills it is always there.
fn react_button(
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
    hovered: Option<RwSignal<bool>>,
) -> impl IntoView {
    let menu = move || reaction_menu(state, backend.clone(), node_id.clone());
    tip(
        Container::new(
            sidebar_glyph(SidebarIcon::Smiley, theme::FG_MUTED)
                .style(|s| s.size(14.0, 14.0).pointer_events_none()),
        )
        .popout_menu(menu)
        .style(move |s| {
            clickable(s)
                .size(24.0, 24.0)
                .items_center()
                .justify_center()
                .border_radius(12.0)
                .hover(|s| s.background(theme::BADGE_BG))
                .apply_if(hovered.is_some_and(|h| !h.get()), |s| s.hide())
        })
        .debug_name("react-button"),
        "React".into(),
    )
}

/// The reactions under an entry's text: a pill per kind anyone left,
/// each with its count, the reader's own washed blue, and the picker at
/// the end. Nothing at all while nobody has reacted; the byline's face
/// is the way in then. A click on a pill toggles the reader's.
fn reactions_row(state: ReviewState, backend: Rc<Backend>, node_id: String) -> impl IntoView {
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
            |g| (g.kind, g.count, g.viewer_reacted),
            move |g| reaction_pill(state, pill_backend.clone(), pill_node.clone(), g),
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
    state: ReviewState,
    backend: Rc<Backend>,
    node_id: String,
    group: ReactionGroup,
) -> impl IntoView {
    let ReactionGroup {
        kind,
        count,
        viewer_reacted: mine,
    } = group;
    let click_node = node_id.clone();
    let busy = Memo::new(move |_| state.is_reacting(&node_id, kind));
    tip(
        Stack::horizontal((
            Label::derived(move || kind.emoji().to_string()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .selectable(false)
                    .pointer_events_none()
            }),
            Label::derived(move || count.to_string()).style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_weight(theme::weight_emphasis())
                    .color(if mine { theme::UNREAD } else { theme::FG_MUTED })
                    .margin_left(5.0)
                    .margin_bottom(theme::OPTICAL_TEXT_LIFT)
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
                .border_color(if mine { theme::REACTED_BORDER } else { theme::BORDER })
                .background(if mine { theme::REACTED_BG } else { theme::CANVAS })
                .hover(move |s| {
                    s.background(if mine { theme::REACTED_BG } else { theme::BADGE_BG })
                        .border_color(if mine { theme::UNREAD } else { theme::FG_SUBTLE })
                })
        }),
        kind.label().to_string(),
    )
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
    view.tooltip(move || tip_body(text.clone()))
}

/// The tooltip itself, for a view that shows one on its own terms.
pub fn tip_body(text: impl Fn() -> String + 'static) -> Label {
    Label::derived(text).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::FG)
            .padding_vert(5.0)
            .padding_horiz(8.0)
            .max_width(320.0)
            .text_wrap()
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
            .box_shadow_blur(6.0)
            .box_shadow_v_offset(2.0)
            .box_shadow_color(theme::SHADOW)
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

// ---------------------------------------------------------------------
// GitHub's cards
// ---------------------------------------------------------------------

/// The pull request's or issue's own text, led by who opened it, with the
/// reactions on it under.
#[allow(clippy::too_many_arguments)]
pub fn description_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    author: String,
    avatar_url: String,
    at: Option<jiff::Timestamp>,
    body: String,
    node_id: String,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let who = Byline {
        login: author,
        avatar_url: fixed(avatar_url),
        avatar_size: theme::CARD_AVATAR,
        glyph: None,
        verb: "opened".into(),
        when: at,
    };
    let description = if body.trim().is_empty() {
        Label::derived(|| "No description.".to_string())
            .style(|s| s.font_size(theme::TEXT_BODY).color(theme::FG_SUBTLE))
            .into_any()
    } else {
        markdown::render_linked(&body, state).into_any()
    };
    let react = reactable(&node_id);
    let reactions = reactions_under(state, backend.clone(), &react);
    track_hover(
        card(
            byline_strip(byline(
                app,
                who,
                actions(state, backend, hovered, None, None, react, String::new()),
            )),
            Stack::vertical((
                Label::derived(|| "Description".to_string()).style(|s| {
                    s.font_size(theme::TEXT_SECTION_TITLE)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::FG)
                        .margin_bottom(10.0)
                }),
                description,
                reactions,
            ))
            .style(|s| s.flex_col().width_full())
            .into_any(),
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
    let react = reactable(&comment.node_id);
    let reactions = reactions_under(state, backend.clone(), &react);
    track_hover(
        card(
            byline_strip(byline(
                app,
                who,
                actions(state, backend, hovered, None, Some(ask), react, comment.html_url.clone()),
            )),
            Stack::vertical((markdown::render_linked(&comment.body, state), reactions))
                .style(|s| s.flex_col().width_full())
                .into_any(),
        ),
        hovered,
    )
}

/// The glyph a verdict takes: a green check, a red page of changes, or a
/// muted comment bubble for anything else.
pub fn verdict_glyph(review: &Review) -> (SidebarIcon, Color) {
    match review.state.as_str() {
        "APPROVED" => (SidebarIcon::Check, theme::OPEN),
        "CHANGES_REQUESTED" => (SidebarIcon::ChangesRequested, theme::CLOSED),
        _ => (SidebarIcon::Comment, theme::FG_MUTED),
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
    // A review with no summary has nothing to react to; GitHub allows it,
    // but there would be nothing to draw the pills under.
    let react = summary.as_ref().and_then(|_| reactable(&review.node_id));

    let body = match summary {
        Some(text) => Stack::vertical((
            markdown::render_linked(&text, state),
            reactions_under(state, backend.clone(), &react),
        ))
        .style(|s| s.flex_col().width_full())
        .into_any(),
        None => Empty::new().into_any(),
    };

    track_hover(
        card(
            byline_strip(byline(app, who, actions(state, backend, hovered, None, ask, react, url))),
            body,
        ),
        hovered,
    )
}

/// `path:line` across the top of a nested block, with Outdated when the
/// thread is, and whatever the caller puts at the right. Click the label
/// and Files opens on the file, scrolled to the line.
fn location_row(
    state: ReviewState,
    open_file: OpenFile,
    path: String,
    line: Option<u64>,
    heading: String,
    outdated: bool,
    trailing: floem::AnyView,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || heading.clone())
            .on_event_stop(el::Click, move |_, _| {
                state.show_tab(ItemTab::Files);
                open_file(&path);
                if let Some(line) = line {
                    state
                        .jump_to_line
                        .set(Some(line.saturating_sub(1) as usize));
                }
            })
            .style(|s| {
                clickable(s)
                    .font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(theme::FG_MUTED)
                    .text_ellipsis()
                    .min_width(0.0)
                    .hover(|s| s.color(theme::ACCENT))
            }),
        threads::outdated_pill(outdated),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        trailing,
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .height(30.0)
            .padding_horiz(12.0)
            .background(theme::CANVAS_SIDEBAR)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

/// The row a GitHub thread with no line opens with: the location row's
/// chrome with nothing to name, holding only the thread's menu, so what
/// Perch put under the thread can still be taken out.
fn menu_row(menu: floem::AnyView) -> impl IntoView {
    Stack::horizontal((Empty::new().style(|s| s.flex_grow(1.0_f32)), menu)).style(|s| {
        s.items_center()
            .width_full()
            .height(30.0)
            .padding_horiz(12.0)
            .background(theme::CANVAS_SIDEBAR)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
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
            Tint::Plain => (None, theme::FG),
            Tint::Added => (Some(theme::DIFF_ADDED_BG), theme::FG),
            Tint::Modified => (Some(theme::DIFF_MODIFIED_BG), theme::FG),
            Tint::Removed => (Some(theme::DIFF_DELETED_BG), theme::DIFF_DELETED_FG),
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
            .border_color(theme::BORDER_MUTED)
    })
    .into_any()
}

fn code_row(line: CodeLine) -> impl IntoView {
    let (bg, fg) = line.tint.colors();
    let number = line.number.map(|n| n.to_string()).unwrap_or_default();
    let text = line.text;
    Stack::horizontal((
        Label::derived(move || number.clone()).style(|s| {
            s.width(theme::CODE_ROW_NUMBER_WIDTH)
                .padding_right(10.0)
                .text_align(floem::text::Alignment::End)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::TEXT_BADGE)
                .color(theme::GUTTER_FG)
                .selectable(false)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || text.clone()).style(move |s| {
            s.font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::TEXT_BADGE)
                .color(fg)
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
        match bg {
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
    Label::derived(move || format!("{more} more {}", if more == 1 { "line" } else { "lines" }))
        .style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .color(theme::FG_SUBTLE)
                .padding_left(theme::CODE_ROW_NUMBER_WIDTH)
                .height(theme::CODE_ROW_HEIGHT)
                .items_center()
                .width_full()
                .selectable(false)
                .border_bottom(1.0)
                .border_color(theme::BORDER_MUTED)
        })
        .into_any()
}

// ---------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------

/// One line, no card chrome: what was done to the pull request or the
/// issue. The dot sits where the cards' avatars do, and the text where
/// their logins do. `author` is who opened the pull request, for the
/// Opened line, which GitHub's timeline sends without an actor.
///
/// Everything after the name is the event's own pieces: words, a label
/// in its colours, a commit in the code face, or a reference to open. The
/// line ends with when it happened.
pub fn event_row(app: AppState, event: PullEvent, author: Option<String>) -> impl IntoView {
    let who = match &event.kind {
        // A commit is signed by a name out of git, which is nobody's
        // profile.
        PullEventKind::Committed { author, .. } if event.actor.is_empty() => {
            (!author.is_empty()).then(|| Name::Plain(author.clone()))
        }
        PullEventKind::Opened if event.actor.is_empty() => author.map(Name::Login),
        _ => (!event.actor.is_empty()).then(|| Name::Login(event.actor.clone())),
    };
    let color = match &event.kind {
        PullEventKind::Opened | PullEventKind::Reopened => theme::OPEN,
        PullEventKind::Merged => theme::MERGED,
        PullEventKind::Closed => theme::CLOSED,
        _ => theme::FG_SUBTLE,
    };
    let at = event.at;
    let pieces = event_pieces(&event);
    // Half an avatar puts the dot under the byline avatars; the text then
    // starts where their logins do.
    let dot_left = theme::CARD_AVATAR / 2.0 - theme::EVENT_DOT / 2.0;
    let text_gap = theme::CARD_AVATAR / 2.0 - theme::EVENT_DOT / 2.0 + 9.0;
    Stack::horizontal((
        Empty::new().style(move |s| {
            s.size(theme::EVENT_DOT, theme::EVENT_DOT)
                .border_radius(theme::EVENT_DOT / 2.0)
                .background(color)
                .flex_shrink(0.0_f32)
                .margin_right(text_gap)
        }),
        match who {
            Some(who) => name_label(
                move || who.clone(),
                |s| {
                    s.font_size(theme::TEXT_META)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::FG)
                        .margin_right(4.0)
                },
            )
            .into_any(),
            None => Empty::new().into_any(),
        },
        dyn_stack(
            move || pieces.clone().into_iter().enumerate(),
            |(i, piece)| format!("{i}:{}", piece.text()),
            |(_, piece)| event_piece(piece),
        )
        .style(|s| s.items_center().gap(4.0).min_width(0.0)),
        Label::derived(move || timefmt::relative(at, app.now.get())).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_MUTED)
                .margin_left(4.0)
                .flex_shrink(0.0_f32)
        }),
    ))
    .style(move |s| {
        s.items_center()
            .width_full()
            .height(theme::EVENT_HEIGHT)
            .padding_left(dot_left)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

/// One piece of an event's line.
fn event_piece(piece: Piece) -> floem::AnyView {
    if piece.text().is_empty() {
        return Empty::new().into_any();
    }
    match piece {
        Piece::Text(text) => Label::derived(move || text.clone())
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_MUTED)
                    .text_ellipsis()
                    .min_width(0.0)
            })
            .into_any(),
        Piece::Label(label) => crate::review::summary::label_chip(label.name, label.color)
            .style(|s| s.flex_shrink(0.0_f32))
            .into_any(),
        Piece::Sha(sha) => {
            let short = short_sha(&sha);
            Label::derived(move || short.clone())
                .style(|s| {
                    s.font_family(theme::CODE_FONT_STACK.to_string())
                        .font_size(theme::CODE_FONT_SIZE)
                        .color(theme::FG_MUTED)
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
                        .color(theme::FG)
                        .font_weight(theme::weight_emphasis())
                        .flex_shrink(0.0_f32)
                }),
                Label::derived(move || title.clone()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::FG_MUTED)
                        .text_ellipsis()
                        .min_width(0.0)
                }),
            ))
            .on_event_stop(el::Click, move |_, _| open_in_browser(&url))
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

// ---------------------------------------------------------------------
// The thread card
// ---------------------------------------------------------------------

/// A thread. One of the reader's own opens with "You asked" and keeps a
/// quiet inset rule and the Local chip, so it is distinct from GitHub
/// without breaking the transcript into another framed card. One of
/// GitHub's, or one that starts with a draft on its way there, opens
/// with where it was left: each comment carries its own byline, and the
/// thread's menu sits at the end of that row.
pub fn thread_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    thread: CommentThread,
) -> impl IntoView {
    let id = thread.id.clone();
    let first = thread.messages.first();
    let from_github = first.is_some_and(|m| m.from_github() || m.is_draft());
    let hunk = first
        .filter(|m| m.from_github())
        .and_then(|m| m.diff_hunk.clone());
    let menu = if from_github {
        threads::thread_menu(state, &thread, theme::THREAD_MENU).into_any()
    } else {
        Empty::new().into_any()
    };
    Stack::vertical((
        if from_github {
            Empty::new().into_any()
        } else {
            thread_byline(app, state, &thread).into_any()
        },
        finding_controls(app, state, backend.clone(), thread.id.clone()),
        threads::delete_confirmation(state, backend.clone(), id.clone(), thread.is_github_bound()),
        match thread.anchor.clone() {
            Some(anchor) => anchor_block(
                state,
                backend.clone(),
                open_file,
                anchor,
                thread.outdated,
                hunk,
                menu,
            )
            .into_any(),
            None if from_github && thread.messages.iter().any(|m| m.is_private()) => {
                menu_row(menu).into_any()
            }
            None => Empty::new().into_any(),
        },
        threads::messages_view(app, state, backend.clone(), id.clone(), ThreadFace::Card),
        threads::follow_up(app, state, backend, id, ThreadFace::Card),
    ))
    .style(move |s| {
        let base = s
            .flex_col()
            .width_full()
            .padding_vert(theme::CONVERSATION_ENTRY_PADDING)
            .gap(theme::CARD_GAP)
            .background(theme::CANVAS)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED);
        if from_github {
            base
        } else {
            base.padding_left(16.0).border_left(2.0)
        }
    })
}

/// Provenance and explicit lifecycle controls for a durable review finding.
/// Ordinary user-created threads have no finding record and render no row.
pub(crate) fn finding_controls(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    thread_id: String,
) -> impl IntoView {
    dyn_container(
        move || {
            let item_id = state
                .item
                .with(|item| item.as_ref().map(|item| item.id.clone()))?;
            let summary = match state.historical_findings.get() {
                Some(findings) => findings
                    .iter()
                    .find(|summary| summary.finding.thread_id == thread_id)
                    .cloned(),
                None => app.review_finding_for_thread(&item_id, &thread_id),
            }?;
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
            Some((
                summary,
                state.historical_review.with(Option::is_some),
                original_group,
            ))
        },
        move |summary| match summary {
            Some((summary, historical, original_group)) => {
                let finding = &summary.finding;
                let reviewer = if summary.attributions.is_empty() {
                    match (&summary.backend, &summary.model) {
                        (Some(backend), Some(model)) => format!("{backend} / {model}"),
                        (Some(backend), None) => backend.clone(),
                        _ => "reviewer unavailable".to_string(),
                    }
                } else {
                    summary
                        .attributions
                        .iter()
                        .map(|(backend, model)| format!("{backend} / {model}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let seen = if summary.seen_in_reviews.is_empty() {
                    summary
                        .original_review_number
                        .map(|number| format!("review #{number}"))
                        .unwrap_or_else(|| "original review unavailable".into())
                } else {
                    format!(
                        "seen in {}",
                        summary
                            .seen_in_reviews
                            .iter()
                            .map(|number| format!("#{number}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let verified = finding
                    .last_verified_head_sha
                    .as_deref()
                    .map(|sha| format!("verified at {}", short_revision(sha)))
                    .unwrap_or_else(|| "not yet verified".into());
                let metadata = format!(
                    "{} · {reviewer} · {seen} · {verified}",
                    finding_severity_label(summary.severity)
                );
                let status = if let Some(canonical) = finding.duplicate_of.as_deref() {
                    format!("Linked duplicate of {}", short_finding(canonical))
                } else if let Some(candidate) = finding.possible_duplicate_of.as_deref() {
                    format!("Possible duplicate of {}", short_finding(candidate))
                } else if finding.disposition == perch_github::FindingDisposition::Dismissed {
                    format!(
                        "Dismissed · {}",
                        finding
                            .dismissal_reason
                            .as_deref()
                            .unwrap_or("No reason recorded")
                    )
                } else if finding.verification == perch_github::FindingVerification::Fixed {
                    "Verified fixed".to_string()
                } else {
                    format!(
                        "{} · {}",
                        finding.disposition.name(),
                        finding.verification.name()
                    )
                };

                let finding_id = finding.id.clone();
                let dismiss_backend = backend.clone();
                let dismiss = if finding.disposition == perch_github::FindingDisposition::Open
                    && finding.verification != perch_github::FindingVerification::Fixed
                    && finding.duplicate_of.is_none()
                    && !historical
                {
                    finding_action("Dismiss", move || {
                        dismiss_backend
                            .dismiss_review_finding(&finding_id, "Dismissed by the user in Perch");
                    })
                    .into_any()
                } else {
                    Empty::new().into_any()
                };

                let finding_id = finding.id.clone();
                let reopen_backend = backend.clone();
                let reopen = if finding.disposition == perch_github::FindingDisposition::Dismissed
                    && !historical
                {
                    finding_action("Reopen", move || {
                        reopen_backend.reopen_review_finding(&finding_id);
                    })
                    .into_any()
                } else {
                    Empty::new().into_any()
                };

                let finding_id = finding.id.clone();
                let duplicate_backend = backend.clone();
                let link = match (
                    finding.duplicate_of.as_ref(),
                    finding.possible_duplicate_of.as_ref(),
                ) {
                    (None, Some(candidate)) if !historical => {
                        let candidate = candidate.clone();
                        finding_action("Link duplicate", move || {
                            duplicate_backend.link_review_duplicate(&finding_id, &candidate);
                        })
                        .into_any()
                    }
                    _ => Empty::new().into_any(),
                };

                let finding_id = finding.id.clone();
                let unlink_backend = backend.clone();
                let unlink = if finding.duplicate_of.is_some() && !historical {
                    finding_action("Unlink", move || {
                        unlink_backend.unlink_review_duplicate(&finding_id);
                    })
                    .into_any()
                } else {
                    Empty::new().into_any()
                };

                let original_backend = backend.clone();
                let view_original = match original_group {
                    Some(group) if !historical => {
                        let group_id = group.group.id.clone();
                        let item_id = group.group.item_id.clone();
                        finding_action("View original", move || {
                            let request_id =
                                original_backend.open_review_snapshot(&item_id, &group_id);
                            app.begin_historical_review(state, group.clone(), request_id);
                        })
                        .into_any()
                    }
                    _ => Empty::new().into_any(),
                };

                Stack::vertical((
                    Label::derived(move || metadata.clone()).style(|s| {
                        s.font_size(theme::TEXT_BADGE)
                            .font_weight(theme::weight_emphasis())
                            .color(theme::FG_MUTED)
                            .text_wrap()
                    }),
                    Stack::horizontal((
                        Label::derived(move || status.clone())
                            .style(|s| s.font_size(theme::TEXT_BADGE).color(theme::FG_SUBTLE)),
                        Empty::new().style(|s| s.flex_grow(1.0_f32)),
                        dismiss,
                        reopen,
                        link,
                        unlink,
                        view_original,
                    ))
                    .style(|s| s.items_center().gap(6.0).width_full()),
                ))
                .style(|s| {
                    s.flex_col()
                        .width_full()
                        .gap(5.0)
                        .padding(8.0)
                        .background(theme::CANVAS_SUBTLE)
                        .border_radius(theme::RADIUS)
                })
                .into_any()
            }
            None => Empty::new().into_any(),
        },
    )
}

fn finding_action(label: &'static str, action: impl Fn() + 'static) -> impl IntoView {
    Label::derived(move || label.to_string())
        .on_event_stop(el::Click, move |_, _| action())
        .style(|s| {
            clickable(s)
                .height(24.0)
                .padding_horiz(8.0)
                .items_center()
                .font_size(theme::TEXT_BADGE)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG_MUTED)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .selectable(false)
        })
}

fn finding_severity_label(severity: perch_github::FindingSeverity) -> &'static str {
    match severity {
        perch_github::FindingSeverity::Unknown => "Finding",
        perch_github::FindingSeverity::Note => "Note",
        perch_github::FindingSeverity::Low => "Low",
        perch_github::FindingSeverity::Medium => "Medium",
        perch_github::FindingSeverity::High => "High",
        perch_github::FindingSeverity::Critical => "Critical",
    }
}

fn short_revision(value: &str) -> &str {
    &value[..value.len().min(8)]
}

fn short_finding(value: &str) -> String {
    format!("finding {}", &value[..value.len().min(8)])
}

/// "You asked Sonnet · High", the time, Local, and the thread's menu.
/// Local is left off a thread on its way to GitHub with a draft.
fn thread_byline(app: AppState, state: ReviewState, thread: &CommentThread) -> impl IntoView {
    let local = !thread.is_github_bound();
    let question = thread.messages.first();
    // The run sits on the question while it goes and on the answer once
    // it is done; whichever message has one names the model.
    let run = thread.messages.iter().rev().find_map(|m| m.run.as_ref());
    let asked = match run {
        Some(run) => {
            let effort = perch_agent::Effort::parse(&run.effort)
                .map(|e| e.label().to_string())
                .unwrap_or_else(|| run.effort.clone());
            format!(
                "asked {} \u{00b7} {effort}",
                threads::model_label(&run.model)
            )
        }
        None => "asked".to_string(),
    };
    let at = question.map(|m| m.created_at).unwrap_or(thread.created_at);
    // The reader's own face and login, once GitHub has said who the token
    // belongs to; "You" until then. Read live, since that answer can land
    // after the card is built.
    let viewer = move || app.viewer.get().filter(|(l, _)| !l.is_empty());
    let seed = viewer().map(|(l, _)| l).unwrap_or_else(|| "you".into());
    Stack::horizontal((
        avatar(
            app,
            move || viewer().map(|(_, a)| a).unwrap_or_default(),
            theme::CARD_AVATAR,
            seed,
        )
        .style(|s| s.margin_right(9.0).flex_shrink(0.0_f32)),
        name_label(
            move || match viewer() {
                Some((login, _)) => Name::Login(login),
                None => Name::Plain("You".into()),
            },
            |s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::FG)
            },
        ),
        Label::derived(move || {
            format!("{asked} \u{00b7} {}", timefmt::relative(at, app.now.get()))
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_MUTED)
                .margin_left(6.0)
                .text_ellipsis()
                .min_width(0.0)
        }),
        if local {
            local_chip().into_any()
        } else {
            Empty::new().into_any()
        },
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        threads::thread_menu(state, thread, theme::THREAD_MENU),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// The mark that says this is Perch's and not GitHub's.
fn local_chip() -> impl IntoView {
    tip(
        Container::new(Label::derived(|| "Local".to_string()).style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(theme::weight_emphasis())
                .color(theme::FG_MUTED)
                .selectable(false)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }))
        .style(|s| {
            s.height(theme::CHIP_HEIGHT)
                .padding_horiz(7.0)
                .margin_left(8.0)
                .items_center()
                .justify_center()
                .background(theme::BADGE_BG)
                .border_radius(theme::CHIP_HEIGHT / 2.0)
                .flex_shrink(0.0_f32)
        }),
        "This thread is on this machine and is not on GitHub.".to_string(),
    )
}

/// Where a thread is anchored, and the lines. The lines come from the
/// loaded file, which is asked for when nothing has loaded it yet; a load
/// fills `files` without opening a tab. A thread from GitHub brings the
/// hunk its comment was left on, `hunk`, which stands in for the file
/// when the thread is outdated or the checkout is not at the pull
/// request's head, since GitHub's line numbers are the head's: a root
/// ahead of the pull request, or a checkout left stale, would put the
/// comment's number on another line. The window is then the hunk's: the
/// commented line and the few above it.
fn anchor_block(
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    anchor: Anchor,
    outdated: bool,
    hunk: Option<String>,
    trailing: floem::AnyView,
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
    if from_file() && !loaded.get_untracked() && !state.is_loading(&anchor.path) {
        state.loading.update(|m| {
            m.insert(anchor.path.clone(), ());
        });
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
    Stack::vertical((
        location_row(
            state,
            open_file,
            anchor.path.clone(),
            Some(anchor.start_line as u64),
            anchor.label(),
            outdated,
            trailing,
        ),
        dyn_container(
            move || (loaded.get(), from_file()),
            move |(loaded, from_file)| {
                let file = (loaded && from_file)
                    .then(|| state.file(&lines_anchor.path))
                    .flatten()
                    .filter(|f| !has_hunk || f.scope == perch_git::Scope::Branch);
                match (file, &hunk) {
                    (Some(file), _) => {
                        let (lines, more) =
                            anchored_lines(&file, &lines_anchor, theme::ANCHOR_LINES);
                        Stack::vertical((code_lines(lines), more_row(more)))
                            .style(|s| s.flex_col().width_full())
                            .into_any()
                    }
                    (None, Some(hunk)) => code_lines(hunk.clone()).into_any(),
                    (None, None) => Empty::new().into_any(),
                }
            },
        )
        .style(|s| s.width_full()),
    ))
    .style(|s| s.flex_col().width_full())
    .clip()
    .style(|s| {
        s.width_full()
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
    })
}

// ---------------------------------------------------------------------
// The composer's place
// ---------------------------------------------------------------------

/// A row that opens the composer when nothing is in it: the box the
/// question will go in, with its placeholder, before it is a box.
pub fn ask_box(app: AppState, state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Thread, theme::FG_SUBTLE)
            .style(|s| s.size(13.0, 13.0).margin_right(8.0)),
        Label::derived(move || format!("Ask about this {}…", threads::subject_word(state))).style(
            |s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::FG_SUBTLE)
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
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(14.0)
            .hover(|s| {
                s.background(theme::CANVAS_HOVER)
                    .border_color(theme::BORDER)
                    .cursor(CursorStyle::Text)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
