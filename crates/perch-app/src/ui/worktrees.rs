//! The worktrees screen: every checkout Perch holds, active or archived.
//!
//! Wears the inbox's clothes: the same cards, row height and type scale, so
//! crossing between the two sections feels like turning a page rather than
//! opening another app. The panel beside the rail chooses Active or
//! Archived, the way it chooses Inbox, Saved and Done.

use std::rc::Rc;

use floem::prelude::*;
use floem::style::CursorStyle;
use floem::text::{Alignment, FontWeight};
use floem::views::{
    ClipExt, Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack,
};
use floem::AnyView;
use perch_github::{SubjectState, Worktree};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::state::{AppState, WorktreeSegment, worktree_time};
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{
    avatar, centered_message, count_badge, hairline, icon_button, worktree_menu,
};

/// Opening a worktree, archived or not. Shared by every row.
pub type OpenWorktree = Rc<dyn Fn(&Worktree)>;

pub fn worktrees_screen(
    state: AppState,
    backend: Rc<Backend>,
    on_open: OpenWorktree,
) -> impl IntoView {
    dyn_container(
        move || {
            let segment = state.worktree_segment.get();
            (segment, state.worktree_count(segment) == 0)
        },
        move |(segment, empty)| {
            if empty {
                return empty_state(segment).into_any();
            }
            groups(state, segment, backend.clone(), on_open.clone()).into_any()
        },
    )
    .style(|s| s.size_full().min_height(0.0).background(theme::CANVAS_SUBTLE))
}

fn empty_state(segment: WorktreeSegment) -> impl IntoView {
    let (glyph, title, body) = match segment {
        WorktreeSegment::Active => (
            SidebarIcon::Branch,
            "No worktrees",
            "Open a pull request from the inbox to make one.",
        ),
        WorktreeSegment::Archived => (
            SidebarIcon::Archive,
            "Nothing archived",
            "An archived worktree keeps its branch, and comes back with a click.",
        ),
    };
    centered_message(glyph, move || title.to_string(), move || body.to_string())
}

/// One card per repository, like the inbox's default grouping.
fn groups(
    state: AppState,
    segment: WorktreeSegment,
    backend: Rc<Backend>,
    on_open: OpenWorktree,
) -> impl IntoView {
    dyn_stack(
        move || state.worktree_groups(segment),
        |(repo, rows)| (repo.clone(), rows.iter().map(|w| w.id.clone()).collect::<Vec<_>>()),
        move |(repo, rows)| repo_card(state, repo, rows, backend.clone(), on_open.clone()),
    )
    .style(|s| s.flex_col().width_full().gap(16.0).padding(16.0))
    .scroll()
    .style(|s| s.size_full().min_height(0.0))
}

fn repo_card(
    state: AppState,
    repo: String,
    rows: Vec<Worktree>,
    backend: Rc<Backend>,
    on_open: OpenWorktree,
) -> impl IntoView {
    let count = rows.len();

    Stack::vertical((
        card_header(state, repo, count),
        dyn_stack(
            move || rows.clone().into_iter().enumerate(),
            |(i, w)| (*i, w.id.clone(), w.head_sha.clone(), w.last_opened_at, w.archived_at),
            move |(i, worktree)| {
                Stack::vertical((
                    if i == 0 { Empty::new().into_any() } else { hairline().into_any() },
                    row(state, worktree, backend.clone(), on_open.clone()),
                ))
                .style(|s| s.width_full())
            },
        )
        .style(|s| s.flex_col().width_full()),
    ))
    // Sizing goes on the stack: `clip` wraps it, and a style after the
    // wrap lands on the wrapper.
    .style(|s| s.width_full().flex_col())
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

fn card_header(state: AppState, repo: String, count: usize) -> impl IntoView {
    let seed = repo.clone();
    let avatar_repo = repo.clone();
    let title = repo.clone();

    Stack::horizontal((
        // The owner's avatar, when the inbox has already fetched it. A
        // worktree opened offline still gets the initial.
        avatar(
            state,
            move || state.repo_avatar_url(&avatar_repo).unwrap_or_default(),
            18.0,
            seed,
        )
        .style(|s| s.margin_right(9.0).flex_shrink(0.0_f32)),
        Label::derived(move || title.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG)
                .text_ellipsis()
                .min_width(0.0)
                .margin_right(8.0)
        }),
        count_badge(move || count),
    ))
    .style(|s| {
        s.height(44.0)
            .width_full()
            .items_center()
            .padding_horiz(14.0)
            .background(theme::CANVAS_SIDEBAR)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
            .selectable(false)
    })
}

fn row(
    state: AppState,
    worktree: Worktree,
    backend: Rc<Backend>,
    on_open: OpenWorktree,
) -> impl IntoView {
    let hover_id = worktree.id.clone();
    let leave_id = worktree.id.clone();
    let open_target = worktree.clone();
    let archived = worktree.is_archived();
    let menu_path = worktree.path.clone();
    let menu_id = worktree.id.clone();
    let menu_backend = backend.clone();

    let content = Stack::horizontal((
        state_icon(&worktree),
        text_column(&worktree),
        state_pill(&worktree),
        row_actions(state, worktree.clone(), backend),
        author_avatar(state, &worktree),
        timestamp(state, &worktree),
    ))
    .on_event_stop(el::Click, move |_, _| on_open(&open_target))
    .on_event_cont(el::PointerEnter, move |_, _| {
        state.hovered_worktree.set(Some(hover_id.clone()));
    })
    .on_event_cont(el::PointerLeave, move |_, _| {
        state.hovered_worktree.update(|h| {
            if h.as_deref() == Some(leave_id.as_str()) {
                *h = None;
            }
        });
    })
    .style(move |s| {
        crate::ui::widgets::clickable(s)
            .height(theme::ROW_HEIGHT)
            .width_full()
            .items_center()
            .padding_horiz(14.0)
            .hover(|s| s.background(theme::CANVAS_SUBTLE))
    });

    if archived {
        // Nothing on disk to reveal or open, so no menu.
        content.into_any()
    } else {
        content
            .context_menu(move || {
                let backend = menu_backend.clone();
                let id = menu_id.clone();
                worktree_menu(menu_path.clone(), Some(Rc::new(move || backend.archive_worktree(&id))))
            })
            .into_any()
    }
}

/// Open, draft, merged or closed, from the snapshot.
fn subject_state(worktree: &Worktree) -> Option<SubjectState> {
    let pr = worktree.pull_request.as_ref()?;
    Some(if pr.merged {
        SubjectState::PrMerged
    } else if !pr.is_open() {
        SubjectState::PrClosed
    } else if pr.draft {
        SubjectState::PrDraft
    } else {
        SubjectState::PrOpen
    })
}

fn state_icon(worktree: &Worktree) -> impl IntoView {
    let state = subject_state(worktree);
    Container::new(match state {
        Some(state) => floem::views::svg(move || icons::state_icon(state))
            .style(move |s| s.size(16.0, 16.0).color(icons::state_color(state)))
            .into_any(),
        None => crate::ui::widgets::sidebar_glyph(SidebarIcon::Branch, theme::FG_SUBTLE)
            .into_any(),
    })
    .style(|s| s.size(16.0, 16.0).margin_right(12.0).flex_shrink(0.0_f32))
}

fn text_column(worktree: &Worktree) -> impl IntoView {
    let title = match (&worktree.pull_request, worktree.pr_number) {
        (Some(pr), _) => format!("#{}  {}", pr.number, pr.title),
        (None, Some(n)) => format!("#{n}"),
        (None, None) => worktree.branch.clone(),
    };
    let branch = worktree.branch.clone();
    let dir = worktree
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    Stack::vertical((
        Label::derived(move || title.clone()).style(|s| {
            s.font_size(theme::TEXT_TITLE)
                .color(theme::FG)
                .text_ellipsis()
                .max_width_full()
                .margin_bottom(3.0)
        }),
        Stack::horizontal((
            Label::derived(move || branch.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(theme::FG_MUTED)
                    .text_ellipsis()
            }),
            Label::derived(move || format!("\u{00b7}  {dir}")).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .margin_left(8.0)
                    .text_ellipsis()
                    .min_width(0.0)
            }),
        ))
        .style(|s| s.items_center().max_width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .flex_grow(1.0_f32)
            .min_width(0.0)
            .justify_center()
            .margin_right(12.0)
            .selectable(false)
    })
}

/// A tinted word for the pull request's state, so a merged or closed
/// worktree reads at a glance as one that can probably go.
fn state_pill(worktree: &Worktree) -> impl IntoView {
    let state = subject_state(worktree);
    let (text, color) = match state {
        Some(SubjectState::PrMerged) => ("Merged", theme::MERGED),
        Some(SubjectState::PrClosed) => ("Closed", theme::CLOSED),
        Some(SubjectState::PrDraft) => ("Draft", theme::DRAFT),
        Some(_) => ("Open", theme::OPEN),
        None => ("No pull request", theme::FG_SUBTLE),
    };

    Container::new(Label::derived(move || text.to_string()).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(FontWeight::MEDIUM)
            .color(color)
            .selectable(false)
            .margin_bottom(theme::OPTICAL_TEXT_LIFT)
    }))
    .style(move |s| {
        s.height(theme::PILL_HEIGHT)
            .padding_horiz(9.0)
            .border(1.0)
            .border_color(color.with_alpha(0.45))
            .border_radius(theme::PILL_HEIGHT / 2.0)
            .items_center()
            .justify_center()
            .flex_shrink(0.0_f32)
            .margin_right(10.0)
    })
}

/// Archive on an active row; Restore and Forget on an archived one. Shown
/// on hover, with the space held so the row does not reflow.
fn row_actions(state: AppState, worktree: Worktree, backend: Rc<Backend>) -> impl IntoView {
    let archived = worktree.is_archived();
    let over_a = worktree.id.clone();
    let hovering_a = move || state.hovered_worktree.with(|h| h.as_deref() == Some(over_a.as_str()));
    let over_b = worktree.id.clone();
    let hovering_b = move || state.hovered_worktree.with(|h| h.as_deref() == Some(over_b.as_str()));

    // First slot: the menu on an active row, Restore on an archived one.
    let first: Box<dyn Fn() -> AnyView> = if archived {
        let id = worktree.id.clone();
        let backend = backend.clone();
        Box::new(move || {
            let id = id.clone();
            let backend = backend.clone();
            icon_button(|| SidebarIcon::Undo, || theme::FG_SUBTLE, move || {
                backend.restore_worktree(&id)
            })
            .into_any()
        })
    } else {
        let path = worktree.path.clone();
        let id = worktree.id.clone();
        let backend = backend.clone();
        Box::new(move || {
            let backend = backend.clone();
            let id = id.clone();
            let path = path.clone();
            more_button(move || {
                let backend = backend.clone();
                let id = id.clone();
                worktree_menu(path.clone(), Some(Rc::new(move || backend.archive_worktree(&id))))
            })
            .into_any()
        })
    };

    // Second slot: Archive, or Forget.
    let id = worktree.id.clone();
    let second: Box<dyn Fn() -> AnyView> = Box::new(move || {
        let id = id.clone();
        let backend = backend.clone();
        icon_button(
            move || if archived { SidebarIcon::Trash } else { SidebarIcon::Archive },
            || theme::FG_SUBTLE,
            move || {
                if archived {
                    backend.forget_worktree(&id)
                } else {
                    backend.archive_worktree(&id)
                }
            },
        )
        .into_any()
    });

    Stack::horizontal((
        dyn_container(hovering_a, move |on| if on { first() } else { Empty::new().into_any() })
            .style(|s| s.size(26.0, 26.0)),
        dyn_container(hovering_b, move |on| if on { second() } else { Empty::new().into_any() })
            .style(|s| s.size(26.0, 26.0)),
    ))
    .style(|s| s.items_center().gap(2.0).margin_right(8.0).flex_shrink(0.0_f32))
}

/// The `...` button: a native menu of places to take the worktree.
fn more_button(menu: impl Fn() -> floem::menu::Menu + 'static) -> impl IntoView {
    Container::new(
        crate::ui::widgets::sidebar_glyph(SidebarIcon::Ellipsis, theme::FG_SUBTLE)
            // The menu looks only at the deepest view under the pointer, so
            // the glyph must not be it.
            .style(|s| s.pointer_events_none()),
    )
    .popout_menu(menu)
    // The row underneath is a button too.
    .on_event_stop(el::Click, |_, _| {})
    .style(|s| {
        crate::ui::widgets::clickable(s)
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(13.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
    })
}

fn author_avatar(state: AppState, worktree: &Worktree) -> impl IntoView {
    let (login, url) = worktree
        .pull_request
        .as_ref()
        .map(|pr| (pr.author_login.clone(), pr.author_avatar_url.clone()))
        .unwrap_or_else(|| (worktree.repo_full_name.clone(), String::new()));

    avatar(state, move || url.clone(), 20.0, login)
        .style(|s| s.size(20.0, 20.0).flex_shrink(0.0_f32).margin_right(12.0))
}

/// When it was last opened, or for an archived row, when it was put away.
fn timestamp(state: AppState, worktree: &Worktree) -> impl IntoView {
    let when = worktree_time(worktree);
    let archived = worktree.is_archived();
    Label::derived(move || {
        let ago = timefmt::relative(when, state.now.get());
        if archived { format!("archived {ago}") } else { ago }
    })
    .style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::FG_SUBTLE)
            .width(130.0)
            .flex_shrink(0.0_f32)
            .text_align(Alignment::End)
            .text_ellipsis()
            .selectable(false)
    })
}
