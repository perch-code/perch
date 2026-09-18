//! The cards on Conversation: the description, each comment and review
//! from GitHub, a line for each event, and each local thread. The chrome
//! is what the issue view's cards had, and the issue view is now a caller.
//!
//! A local thread and a GitHub comment must never look like one. Perch
//! does not write to GitHub, so a card that could be taken for a comment
//! on the pull request would be a lie. A GitHub card is white with a
//! byline strip across the top; a thread card sits on the page's own grey
//! inside a dashed border, with a Local chip in its byline that says on
//! hover what Local means.

use std::rc::Rc;

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Container, Decorators, Empty, Label, Stack, TooltipExt, dyn_container, dyn_stack};
use perch_git::{LineStatus, Row};
use perch_github::{Anchor, CommentThread, IssueComment, PullEvent, PullEventKind, Review, ReviewComment};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::markdown;
use crate::review::model::{ItemTab, LoadedFile, ReviewState};
use crate::review::threads::{self, ThreadFace};
use crate::review::timeline::{Card, LineThread, event_name, verdict};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{avatar, clickable, icon_button, open_in_browser, sidebar_glyph};

/// What a card's row in the column is keyed by. A row with the same key is
/// kept as it is, so the key changes with anything the card shows that
/// GitHub can edit; a thread's messages are read live and stay out of it.
pub fn key(card: &Card) -> String {
    match card {
        // The time lands with the discussion, after a first paint from
        // the snapshot, and an edit can keep a body's length.
        Card::Description { body, at, .. } => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            body.hash(&mut h);
            format!("description:{:?}:{:x}", at, h.finish())
        }
        Card::Comment(c) => format!("comment:{}:{:?}", c.id, c.updated_at),
        Card::Review { review, comments } => {
            let id = review.as_ref().map(|r| r.id).or_else(|| comments.first().map(|t| t.comment.id)).unwrap_or(0);
            let edited = comments
                .iter()
                .flat_map(|t| std::iter::once(&t.comment).chain(t.replies.iter()))
                .filter_map(|c| c.updated_at)
                .max();
            let count: usize = comments.iter().map(|t| 1 + t.replies.len()).sum();
            format!("review:{id}:{count}:{edited:?}")
        }
        Card::Event(e) => format!("event:{}:{}", event_name(&e.kind), e.at),
        Card::Thread(t) => format!("thread:{}:{}", t.id, t.outdated),
    }
}

// ---------------------------------------------------------------------
// Chrome
// ---------------------------------------------------------------------

/// One card: a byline strip on top, the text below.
pub fn card(header: impl IntoView + 'static, body: floem::AnyView) -> impl IntoView {
    Stack::vertical((
        header,
        Container::new(body).style(|s| s.width_full().padding(theme::CARD_PADDING)),
    ))
    .style(|s| s.flex_col().width_full())
    .clip()
    .style(|s| {
        s.width_full()
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS_LG)
            .box_shadow_blur(4.0)
            .box_shadow_v_offset(1.0)
            .box_shadow_color(theme::SHADOW)
    })
}

/// The strip across the top of a card, with what a byline holds inside.
fn byline_strip(content: impl IntoView + 'static) -> impl IntoView {
    Container::new(content).style(|s| {
        s.items_center()
            .width_full()
            .height(theme::BYLINE_HEIGHT)
            .padding_horiz(theme::CARD_PADDING)
            .background(theme::CANVAS_SIDEBAR)
            .border_bottom(1.0)
            .border_color(theme::BORDER_MUTED)
    })
}

/// Who, with what glyph, did what, when.
struct Byline {
    login: String,
    avatar_url: String,
    avatar_size: f64,
    /// A verdict's glyph between the avatar and the login.
    glyph: Option<(SidebarIcon, Color)>,
    verb: String,
    when: Option<jiff::Timestamp>,
}

/// Avatar, login, what they did and when, and whatever the caller puts at
/// the right: the permalink and Ask about this.
fn byline(app: AppState, who: Byline, trailing: floem::AnyView) -> impl IntoView {
    let Byline { login, avatar_url, avatar_size, glyph, verb, when } = who;
    let seed = login.clone();
    Stack::horizontal((
        avatar(app, move || avatar_url.clone(), avatar_size, seed).style(|s| s.margin_right(9.0).flex_shrink(0.0_f32)),
        match glyph {
            Some((icon, color)) => sidebar_glyph(icon, color).style(|s| s.size(14.0, 14.0).margin_right(5.0)).into_any(),
            None => Empty::new().into_any(),
        },
        Label::derived(move || login.clone()).style(|s| {
            s.font_size(theme::TEXT_META).font_weight(FontWeight::SEMI_BOLD).color(theme::FG)
        }),
        Label::derived(move || match when {
            Some(at) if verb.is_empty() => timefmt::relative(at, app.now.get()),
            Some(at) => format!("{verb} {}", timefmt::relative(at, app.now.get())),
            None => verb.clone(),
        })
        .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_MUTED).margin_left(6.0).text_ellipsis().min_width(0.0)),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        trailing,
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// The right end of a GitHub byline: Ask about this while the card is
/// hovered, and the permalink always.
fn actions(hovered: RwSignal<bool>, ask: Option<Rc<dyn Fn()>>, url: String) -> floem::AnyView {
    Stack::horizontal((
        match ask {
            Some(ask) => ask_button(hovered, ask).into_any(),
            None => Empty::new().into_any(),
        },
        if url.is_empty() {
            Empty::new().into_any()
        } else {
            icon_button(|| SidebarIcon::Link, || theme::FG_SUBTLE, move || open_in_browser(&url))
                .style(|s| s.size(24.0, 24.0).margin_right(-4.0))
                .into_any()
        },
    ))
    .style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32))
    .into_any()
}

/// "Ask about this", drawn only while the card is under the pointer.
/// Hidden rather than absent, so the byline keeps its shape.
fn ask_button(hovered: RwSignal<bool>, ask: Rc<dyn Fn()>) -> impl IntoView {
    Stack::horizontal((
        sidebar_glyph(SidebarIcon::Thread, theme::FG_MUTED).style(|s| s.size(12.0, 12.0).margin_right(5.0)),
        Label::derived(|| "Ask about this".to_string()).style(|s| {
            s.font_size(theme::TEXT_BADGE).color(theme::FG_MUTED).margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| ask())
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
fn track_hover<V: IntoView + 'static>(view: V, hovered: RwSignal<bool>) -> impl IntoView {
    view.into_intermediate()
        .on_event_cont(el::PointerEnter, move |_, _| hovered.set(true))
        .on_event_cont(el::PointerLeave, move |_, _| hovered.set(false))
}

/// A view with a line of explanation on hover, in the window's overlay so
/// nothing drawn after it can cover it.
pub fn tip<V: IntoView + 'static>(view: V, text: String) -> impl IntoView {
    view.tooltip(move || {
        let text = text.clone();
        Label::derived(move || text.clone()).style(|s| {
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
    })
}

/// The avatar GitHub sent for a login, from whichever payload carries one:
/// the author, a conversation comment, a requested reviewer. A review and
/// a line comment come with none of their own, and would otherwise show
/// an initial beside the same person's picture.
pub fn avatar_url_for(state: ReviewState, login: &str) -> String {
    state
        .pull_request
        .with_untracked(|p| {
            let p = p.as_ref()?;
            if p.author_login == login {
                return Some(p.author_avatar_url.clone());
            }
            p.requested_reviewers.iter().find(|r| r.login == login).map(|r| r.avatar_url.clone())
        })
        .or_else(|| state.issue.with_untracked(|i| i.as_ref().filter(|i| i.author_login == login).map(|i| i.author_avatar_url.clone())))
        .or_else(|| {
            state.discussion.with_untracked(|d| {
                d.as_ref().and_then(|d| d.comments.iter().find(|c| c.user == login).map(|c| c.user_avatar_url.clone()))
            })
        })
        .or_else(|| state.issue_comments.with_untracked(|c| c.iter().find(|c| c.user == login).map(|c| c.user_avatar_url.clone())))
        .filter(|u| !u.is_empty())
        .unwrap_or_default()
}

/// The composer, with a comment quoted above the draft and, for a line
/// comment, anchored to its line. On Conversation the composer is laid in
/// place and scrolls to itself.
fn ask_about(app: AppState, state: ReviewState, anchor: Option<Anchor>, author: &str, url: &str, body: &str) {
    let quote = crate::ask::quote_for(author, url, body);
    state.open_composer(None, anchor, quote, threads::defaults(app));
}

// ---------------------------------------------------------------------
// GitHub's cards
// ---------------------------------------------------------------------

/// The pull request's or issue's own text, led by who opened it.
pub fn description_card(app: AppState, author: String, avatar_url: String, at: Option<jiff::Timestamp>, body: String) -> impl IntoView {
    let who = Byline { login: author, avatar_url, avatar_size: theme::CARD_AVATAR, glyph: None, verb: "opened".into(), when: at };
    card(
        byline_strip(byline(app, who, Empty::new().into_any())),
        if body.trim().is_empty() {
            Label::derived(|| "No description.".to_string())
                .style(|s| s.font_size(theme::TEXT_BODY).color(theme::FG_SUBTLE))
                .into_any()
        } else {
            markdown::render(&body).into_any()
        },
    )
}

/// One conversation comment.
pub fn comment_card(app: AppState, state: ReviewState, comment: IssueComment) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let who = Byline {
        login: comment.user.clone(),
        avatar_url: comment.user_avatar_url.clone(),
        avatar_size: theme::CARD_AVATAR,
        glyph: None,
        verb: "commented".into(),
        when: Some(comment.created_at),
    };
    let (author, url, body) = (comment.user.clone(), comment.html_url.clone(), comment.body.clone());
    let ask: Rc<dyn Fn()> = Rc::new(move || ask_about(app, state, None, &author, &url, &body));
    track_hover(
        card(
            byline_strip(byline(app, who, actions(hovered, Some(ask), comment.html_url.clone()))),
            markdown::render(&comment.body).into_any(),
        ),
        hovered,
    )
}

/// The glyph a verdict takes: a green check, a red page of changes, or a
/// muted comment bubble for anything else.
pub fn verdict_glyph(review: Option<&Review>) -> (SidebarIcon, Color) {
    match review.map(|r| r.state.as_str()) {
        Some("APPROVED") => (SidebarIcon::Check, theme::OPEN),
        Some("CHANGES_REQUESTED") => (SidebarIcon::ChangesRequested, theme::CLOSED),
        _ => (SidebarIcon::Comment, theme::FG_MUTED),
    }
}

/// A review: its verdict and summary, then the line comments that belong
/// to it, each in a block of its own headed by where it was left.
pub fn review_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    review: Option<Review>,
    comments: Vec<LineThread>,
) -> impl IntoView {
    let hovered = RwSignal::new(false);
    // A card with no review is a line comment left on its own; it reads
    // as its author's.
    let login = review
        .as_ref()
        .map(|r| r.user.clone())
        .or_else(|| comments.first().map(|t| t.comment.user.clone()))
        .unwrap_or_default();
    let summary = review.as_ref().map(|r| r.body.clone()).filter(|b| !b.trim().is_empty());
    let url = review.as_ref().map(|r| r.html_url.clone()).unwrap_or_default();
    let who = Byline {
        login: login.clone(),
        avatar_url: avatar_url_for(state, &login),
        avatar_size: theme::CARD_AVATAR,
        glyph: Some(verdict_glyph(review.as_ref())),
        verb: verdict(review.as_ref()).to_string(),
        when: review.as_ref().and_then(|r| r.submitted_at).or_else(|| comments.first().map(|t| t.comment.created_at)),
    };
    let ask = summary.clone().map(|body| {
        let (author, url) = (login.clone(), url.clone());
        Rc::new(move || ask_about(app, state, None, &author, &url, &body)) as Rc<dyn Fn()>
    });

    let body = Stack::vertical((
        match summary {
            Some(text) => markdown::render(&text).into_any(),
            None => Empty::new().into_any(),
        },
        dyn_stack(
            move || comments.clone(),
            |t| t.comment.id,
            move |t| line_thread_block(app, state, backend.clone(), open_file.clone(), t),
        )
        .style(|s| s.flex_col().width_full().gap(theme::CARD_GAP)),
    ))
    .style(|s| s.flex_col().width_full().gap(theme::CARD_GAP));

    track_hover(card(byline_strip(byline(app, who, actions(hovered, ask, url))), body.into_any()), hovered)
}

/// A line comment and its replies under `path:line` and the lines of the
/// hunk it was left on.
/// A review comment and its replies as one block: the lines it was on,
/// then the comments, each with Ask about this. On Conversation inside a
/// review card; on Files in the thread column under On GitHub.
pub fn line_thread_block(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    thread: LineThread,
) -> impl IntoView {
    let first = thread.comment.clone();
    let path = first.path.clone();
    let line = first.line.or(first.original_line);
    let heading = first.location();

    // The lines as the checkout has them, when the comment still lands on
    // it and the file is loaded at the branch head; GitHub's hunk, as of
    // the comment's commit, otherwise. The file is asked for when nothing
    // has loaded it.
    let hunk = hunk_tail(&first.diff_hunk, theme::HUNK_LINES);
    let anchor = anchor_for(state, &first);
    let lines_path = first.path.clone();
    let loaded = Memo::new(move |_| state.files.with(|m| m.contains_key(&lines_path)));
    if anchor.is_some() && !loaded.get_untracked() && !state.is_loading(&first.path) {
        state.loading.update(|m| {
            m.insert(first.path.clone(), ());
        });
        backend.open_file(&first.path);
    }
    let lines_anchor = anchor.clone();
    let lines_view = dyn_container(
        move || loaded.get(),
        move |loaded| {
            let checkout = lines_anchor
                .as_ref()
                .filter(|_| loaded)
                .and_then(|a| state.file(&a.path).map(|f| (f, a.clone())))
                .filter(|(f, _)| f.scope == perch_git::Scope::Branch)
                .map(|(f, a)| anchored_lines(&f, &a, theme::HUNK_LINES).0)
                .filter(|l| !l.is_empty());
            code_lines(checkout.unwrap_or_else(|| hunk.clone())).into_any()
        },
    )
    .style(|s| s.width_full());

    let replies = thread.replies.clone();
    let anchor_of = move |c: &ReviewComment| anchor_for(state, c);
    Stack::vertical((
        location_row(state, open_file, path, line, heading, false),
        lines_view,
        Stack::vertical((
            nested_comment(app, state, first.clone(), anchor_of(&first), false),
            dyn_stack(
                move || replies.clone(),
                |r| r.id,
                move |r| {
                    let anchor = anchor_of(&r);
                    nested_comment(app, state, r, anchor, true)
                },
            )
            .style(|s| s.flex_col().width_full().gap(10.0)),
        ))
        .style(|s| s.flex_col().width_full().gap(10.0).padding(12.0)),
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

/// The line a GitHub comment was left on, as an anchor for a thread about
/// it: the head side unless the comment is on a removed line. `None` when
/// GitHub no longer places it.
fn anchor_for(state: ReviewState, comment: &ReviewComment) -> Option<Anchor> {
    // Anchors are on the head. A comment on a removed line is on the
    // base, and one GitHub no longer places has no line; both go through
    // as the quote alone.
    if comment.side == perch_github::Side::Left {
        return None;
    }
    let line = comment.line? as u32;
    let commit_sha = state
        .item
        .with(|i| i.as_ref().and_then(|i| i.checkout.as_ref()).map(|c| c.head_sha.clone()))
        .or_else(|| state.pull_request.with(|p| p.as_ref().map(|p| p.head_sha.clone())))
        .unwrap_or_default();
    Some(Anchor { path: comment.path.clone(), start_line: line, line, side: comment.side, commit_sha })
}

/// `path:line` across the top of a nested block. Click it and Files opens
/// on the file, scrolled to the line.
fn location_row(
    state: ReviewState,
    open_file: OpenFile,
    path: String,
    line: Option<u64>,
    heading: String,
    outdated: bool,
) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || heading.clone())
            .on_event_stop(el::Click, move |_, _| {
                state.show_tab(ItemTab::Files);
                open_file(&path);
                if let Some(line) = line {
                    state.jump_to_line.set(Some(line.saturating_sub(1) as usize));
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

/// One comment inside a review's block: a small byline, then the body. A
/// reply sits flush under the comment it answers, with a rule down its
/// left.
fn nested_comment(app: AppState, state: ReviewState, comment: ReviewComment, anchor: Option<Anchor>, reply: bool) -> impl IntoView {
    let hovered = RwSignal::new(false);
    let who = Byline {
        login: comment.user.clone(),
        avatar_url: avatar_url_for(state, &comment.user),
        avatar_size: theme::NESTED_AVATAR,
        glyph: None,
        verb: String::new(),
        when: Some(comment.created_at),
    };
    let (author, url, body) = (comment.user.clone(), comment.html_url.clone(), comment.body.clone());
    let ask: Rc<dyn Fn()> = Rc::new(move || ask_about(app, state, anchor.clone(), &author, &url, &body));
    track_hover(
        Stack::vertical((
            byline(app, who, actions(hovered, Some(ask), comment.html_url.clone())),
            markdown::render(&comment.body),
        ))
        .style(move |s| {
            let base = s.flex_col().width_full().gap(6.0);
            if reply {
                base.padding_left(12.0).border_left(theme::REPLY_RULE).border_color(theme::BORDER_MUTED)
            } else {
                base
            }
        }),
        hovered,
    )
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
        lines.push(CodeLine { number, tint, text: text.to_string() });
    }
    let skip = lines.len().saturating_sub(count);
    lines.drain(..skip);
    lines
}

/// The lines under an anchor, from the file as the editor shows it: the
/// head lines from `start_line` to `line`, with any removed lines that sit
/// between them, tinted as the editor tints them. At most `max` of them,
/// and how many more there were.
pub fn anchored_lines(file: &LoadedFile, anchor: &Anchor, max: usize) -> (Vec<CodeLine>, usize) {
    let first = anchor.start_line.saturating_sub(1) as usize;
    let last = anchor.line.saturating_sub(1) as usize;
    let rows = &file.buffer.rows;
    let from = rows.iter().position(|r| r.head_line().is_some_and(|h| h >= first));
    let to = rows.iter().rposition(|r| r.head_line().is_some_and(|h| h <= last));
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
            CodeLine { number, tint, text: text.get(d).copied().unwrap_or_default().to_string() }
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
    .style(|s| s.flex_col().width_full().border_bottom(1.0).border_color(theme::BORDER_MUTED))
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
        let base = s.items_center().width_full().height(theme::CODE_ROW_HEIGHT).padding_right(12.0);
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

/// One line, no card chrome: what the pull request did. The dot sits
/// where the cards' avatars do, and the text where their logins do.
/// `author` is who opened the pull request, for the Opened line.
pub fn event_row(app: AppState, event: PullEvent, author: Option<String>) -> impl IntoView {
    let (actor, name) = match &event.kind {
        PullEventKind::Merged { by: Some(by) } => (Some(by.clone()), event_name(&PullEventKind::Merged { by: None })),
        PullEventKind::Opened => (author, event_name(&event.kind)),
        kind => (None, event_name(kind)),
    };
    let color = match &event.kind {
        PullEventKind::Opened | PullEventKind::Reopened => theme::OPEN,
        PullEventKind::Merged { .. } => theme::MERGED,
        PullEventKind::Closed => theme::CLOSED,
        PullEventKind::Draft | PullEventKind::Ready => theme::FG_SUBTLE,
    };
    let at = event.at;
    // The card's border, its padding and half an avatar put the dot under
    // the avatars; the text then starts where a login does.
    let dot_left = 1.0 + theme::CARD_PADDING + theme::CARD_AVATAR / 2.0 - theme::EVENT_DOT / 2.0;
    let text_gap = theme::CARD_AVATAR / 2.0 - theme::EVENT_DOT / 2.0 + 9.0;
    Stack::horizontal((
        Empty::new().style(move |s| {
            s.size(theme::EVENT_DOT, theme::EVENT_DOT)
                .border_radius(theme::EVENT_DOT / 2.0)
                .background(color)
                .flex_shrink(0.0_f32)
                .margin_right(text_gap)
        }),
        match actor {
            Some(actor) => Label::derived(move || actor.clone())
                .style(|s| {
                    s.font_size(theme::TEXT_META).font_weight(FontWeight::SEMI_BOLD).color(theme::FG).margin_right(4.0)
                })
                .into_any(),
            None => Empty::new().into_any(),
        },
        Label::derived(move || format!("{name} {}", timefmt::relative(at, app.now.get())))
            .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_MUTED).text_ellipsis().min_width(0.0)),
    ))
    .style(move |s| s.items_center().width_full().height(theme::EVENT_HEIGHT).padding_left(dot_left))
}

// ---------------------------------------------------------------------
// The thread card
// ---------------------------------------------------------------------

/// A local thread. Not a GitHub card: the page's grey inside a dashed
/// border, the reader's own avatar, and a Local chip.
pub fn thread_card(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    thread: CommentThread,
) -> impl IntoView {
    let id = thread.id.clone();
    Stack::vertical((
        thread_byline(app, state, &thread),
        threads::delete_confirmation(state, backend.clone(), id.clone()),
        match thread.anchor.clone() {
            Some(anchor) => anchor_block(state, backend.clone(), open_file, anchor, thread.outdated).into_any(),
            None => Empty::new().into_any(),
        },
        threads::messages_view(app, state, backend.clone(), id.clone(), ThreadFace::Card),
        threads::follow_up(app, state, backend, id, ThreadFace::Card),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .padding(theme::CARD_PADDING)
            .gap(theme::CARD_GAP)
            .background(theme::CANVAS_SUBTLE)
            .border(floem::kurbo::Stroke::new(1.0).with_dashes(0.0, theme::THREAD_DASH))
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS_LG)
    })
}

/// "You asked Sonnet · High", the time, Local, and the thread's menu.
fn thread_byline(app: AppState, state: ReviewState, thread: &CommentThread) -> impl IntoView {
    let question = thread.messages.first();
    // The run sits on the question while it goes and on the answer once
    // it is done; whichever message has one names the model.
    let run = thread.messages.iter().rev().find_map(|m| m.run.as_ref());
    let asked = match run {
        Some(run) => {
            let effort = perch_agent::Effort::parse(&run.effort)
                .map(|e| e.label().to_string())
                .unwrap_or_else(|| run.effort.clone());
            format!("asked {} \u{00b7} {effort}", threads::model_label(&run.model))
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
        avatar(app, move || viewer().map(|(_, a)| a).unwrap_or_default(), theme::CARD_AVATAR, seed)
            .style(|s| s.margin_right(9.0).flex_shrink(0.0_f32)),
        Label::derived(move || viewer().map(|(l, _)| l).unwrap_or_else(|| "You".into())).style(|s| {
            s.font_size(theme::TEXT_META).font_weight(FontWeight::SEMI_BOLD).color(theme::FG)
        }),
        Label::derived(move || format!("{asked} \u{00b7} {}", timefmt::relative(at, app.now.get())))
            .style(|s| s.font_size(theme::TEXT_META).color(theme::FG_MUTED).margin_left(6.0).text_ellipsis().min_width(0.0)),
        local_chip(),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        threads::thread_menu(state, thread.id.clone()),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// The mark that says this is Perch's and not GitHub's.
fn local_chip() -> impl IntoView {
    tip(
        Container::new(Label::derived(|| "Local".to_string()).style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(FontWeight::MEDIUM)
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

/// Where a thread is anchored, and the lines: the same block a review
/// comment shows its lines in. The lines come from the loaded file, which
/// is asked for when nothing has loaded it yet; a load fills `files`
/// without opening a tab.
fn anchor_block(
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    anchor: Anchor,
    outdated: bool,
) -> impl IntoView {
    let path = anchor.path.clone();
    let loaded = Memo::new(move |_| state.files.with(|m| m.contains_key(&path)));
    if !loaded.get_untracked() && !state.is_loading(&anchor.path) {
        state.loading.update(|m| {
            m.insert(anchor.path.clone(), ());
        });
        backend.open_file(&anchor.path);
    }
    let lines_anchor = anchor.clone();
    Stack::vertical((
        location_row(state, open_file, anchor.path.clone(), Some(anchor.start_line as u64), anchor.label(), outdated),
        dyn_container(
            move || loaded.get(),
            move |loaded| {
                if !loaded {
                    return Empty::new().into_any();
                }
                let Some(file) = state.file(&lines_anchor.path) else {
                    return Empty::new().into_any();
                };
                let (lines, more) = anchored_lines(&file, &lines_anchor, theme::ANCHOR_LINES);
                Stack::vertical((code_lines(lines), more_row(more))).style(|s| s.flex_col().width_full()).into_any()
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
        sidebar_glyph(SidebarIcon::Thread, theme::FG_SUBTLE).style(|s| s.size(13.0, 13.0).margin_right(8.0)),
        Label::derived(move || format!("Ask about this {}…", threads::subject_word(state))).style(|s| {
            s.font_size(theme::TEXT_BODY).color(theme::FG_SUBTLE).selectable(false)
        }),
    ))
    .on_event_stop(el::Click, move |_, _| {
        state.open_composer(None, None, String::new(), threads::defaults(app));
    })
    .style(|s| {
        s.items_center()
            .width_full()
            .height(theme::ASK_BOX_HEIGHT)
            .padding_horiz(theme::CARD_PADDING)
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS_LG)
            .hover(|s| s.border_color(theme::ACCENT_RING).cursor(CursorStyle::Text))
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
                CodeLine { number: Some(41), tint: Tint::Removed, text: "let b = 2;".into() },
                CodeLine { number: Some(41), tint: Tint::Added, text: "let b = 3;".into() },
                CodeLine { number: Some(42), tint: Tint::Added, text: "let c = 4;".into() },
                CodeLine { number: Some(43), tint: Tint::Plain, text: "let d = 5;".into() },
            ]
        );
        // The whole hunk when it is short, and nothing from nothing.
        assert_eq!(hunk_tail(hunk, 10).len(), 5);
        assert_eq!(hunk_tail(hunk, 10)[0].number, Some(40), "a context line counts on the head side");
        assert!(hunk_tail("", 4).is_empty());
        assert!(hunk_tail("@@ -1 +1 @@", 4).is_empty());
    }

    #[test]
    fn a_hunk_without_a_header_has_lines_but_no_numbers() {
        let lines = hunk_tail(" a\n+b", 4);
        assert_eq!(lines.iter().map(|l| l.number).collect::<Vec<_>>(), [None, None]);
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
        );
        let anchor = |start, line| Anchor { path: "src/x.rs".into(), start_line: start, line, side: perch_github::Side::Right, commit_sha: "h".into() };
        let (lines, more) = anchored_lines(&file, &anchor(1, 3), 8);
        assert_eq!(more, 0);
        assert_eq!(
            lines.iter().map(|l| (l.number, l.tint, l.text.as_str())).collect::<Vec<_>>(),
            [(Some(1), Tint::Plain, "a"), (Some(2), Tint::Removed, "b"), (Some(2), Tint::Modified, "b2"), (Some(3), Tint::Plain, "c")]
        );
        let (lines, more) = anchored_lines(&file, &anchor(1, 4), 2);
        assert_eq!((lines.len(), more), (2, 3), "cut to the cap, and the rest counted");
        assert!(anchored_lines(&file, &anchor(40, 50), 8).0.is_empty(), "past the end is nothing");
    }

    #[test]
    fn the_key_follows_what_github_can_edit_and_not_a_threads_messages() {
        use crate::review::timeline::tests::at;
        let comment = |edited: Option<&str>| {
            Card::Comment(IssueComment {
                id: 7,
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "q".into(),
                created_at: at("2026-09-15T09:14:00Z"),
                updated_at: edited.map(at),
                html_url: String::new(),
            })
        };
        assert_ne!(key(&comment(None)), key(&comment(Some("2026-09-16T09:14:00Z"))));
        let event = |kind| Card::Event(PullEvent { kind, at: at("2026-09-15T09:14:00Z") });
        assert_ne!(key(&event(PullEventKind::Opened)), key(&event(PullEventKind::Closed)));
    }
}
