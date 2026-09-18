//! Left rail: views, reason filters, and the repository list.

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack};

use crate::filter::{ReasonFilter, View};
use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::state::{AppState, Screen, WorktreeSegment};
use crate::theme;
use crate::ui::widgets::{avatar, count_badge, section_heading, sidebar_row};
use crate::ui::worktrees::OpenWorktree;
use perch_github::Worktree;

/// The navigation panel: views, reason filters, and the repository list.
///
/// Sits beside the activity rail and hides when the rail's current entry is
/// clicked, the way an editor's explorer does.
pub fn nav_panel(state: AppState, review: ReviewState, on_open: OpenWorktree) -> impl IntoView {
    // A review belongs to the worktrees section, whichever screen it was
    // opened from, so the panel beside it lists worktrees. The rail entry
    // follows the same rule, and the two must agree.
    dyn_container(
        move || {
            if review.is_showing() {
                Screen::Worktrees
            } else {
                state.screen.get()
            }
        },
        move |screen| match screen {
            Screen::Threads => Stack::vertical((
                Empty::new().style(|s| s.height(8.0).width_full().flex_shrink(0.0_f32)),
                views_section(state, review),
                section_heading("Filters"),
                filters_section(state),
                section_heading("Repositories"),
                repos_section(state),
            ))
            .style(|s| s.size_full().flex_col())
            .into_any(),
            Screen::Worktrees => worktrees_panel(state, review, on_open.clone()).into_any(),
        },
    )
    .style(|s| {
        s.width(theme::SIDEBAR_WIDTH)
            .height_full()
            .flex_col()
            .background(theme::CANVAS_SIDEBAR)
            .border_right(1.0)
            .border_color(theme::BORDER)
    })
}

/// Inbox, Saved and Done, as three rows.
///
/// Whichever is open wears the card, in the accent, with a bold label: the
/// same treatment the worktree rows give the checkout being read, so the two
/// panels say "you are here" the same way.
fn views_section(state: AppState, review: ReviewState) -> impl IntoView {
    Stack::vertical((
        view_row(state, review, View::Inbox, SidebarIcon::Inbox),
        view_row(state, review, View::Saved, SidebarIcon::Bookmark),
        view_row(state, review, View::Done, SidebarIcon::Check),
    ))
    .style(|s| s.width_full().flex_col().gap(1.0))
}

fn view_row(
    state: AppState,
    review: ReviewState,
    view: View,
    icon: SidebarIcon,
) -> impl IntoView {
    use floem::peniko::Color;
    use floem::text::FontWeight;

    let selected = move || state.view.get() == view && !review.is_showing();
    let count = move || {
        let c = state.counts();
        match view {
            // The inbox badge counts what is still asking for attention, so
            // dismissing a thread quiets it.
            View::Inbox => c.unread,
            View::Saved => c.saved,
            View::Done => c.done,
        }
    };

    let content = Stack::horizontal((
        dyn_container(
            move || if selected() { theme::ACCENT } else { theme::FG_MUTED },
            move |c| crate::ui::widgets::sidebar_glyph(icon, c).into_any(),
        )
        .style(|s| s.size(16.0, 16.0)),
        Label::derived(move || view.label()).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(if selected() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                .color(if selected() { theme::FG } else { theme::FG_MUTED })
                .margin_left(9.0)
                .flex_grow(1.0_f32)
        }),
        dyn_container(count, move |n| {
            if n == 0 {
                Empty::new().into_any()
            } else {
                count_badge(move || n).into_any()
            }
        }),
    ))
    .style(|s| s.items_center().width_full());

    Container::new(content)
        .on_event_stop(el::Click, move |_, _| {
            // Doubles as the way back from a pull request.
            state.show_screen(review, Screen::Threads);
            // Filters carry over, the way the web keeps a search when you
            // cross from the inbox into Done. Clearing them here made this a
            // reset button nobody asked for.
            state.set_view(view);
        })
        .style(move |s| {
            let base = crate::ui::widgets::clickable(s)
                .width_full()
                .height(34.0)
                .items_center()
                .padding_horiz(10.0)
                .margin_horiz(6.0)
                .border_radius(theme::RADIUS);
            if selected() {
                base.background(theme::CANVAS).border(1.0).border_color(theme::BORDER)
            } else {
                base.border(1.0)
                    .border_color(Color::TRANSPARENT)
                    .hover(|s| s.background(theme::CANVAS_HOVER))
            }
        })
}

/// The worktrees panel: every active checkout under its repository, the way
/// an editor lists open projects, with Archived as one row at the bottom.
///
/// The worktree being read is lifted to the top of its repository and wears
/// the card, so the panel always says where the reader is.
fn worktrees_panel(state: AppState, review: ReviewState, on_open: OpenWorktree) -> impl IntoView {
    let open_id = move || review.worktree.with(|w| w.as_ref().map(|w| w.id.clone()));

    let groups = move || {
        let open = open_id();
        let mut groups = state.worktree_groups(WorktreeSegment::Active);
        for (_, rows) in &mut groups {
            if let Some(i) = rows.iter().position(|w| Some(&w.id) == open.as_ref()) {
                let row = rows.remove(i);
                rows.insert(0, row);
            }
        }
        groups
    };

    Stack::vertical((
        Empty::new().style(|s| s.height(4.0).width_full().flex_shrink(0.0_f32)),
        dyn_stack(
            groups,
            |(repo, rows)| (repo.clone(), rows.iter().map(|w| w.id.clone()).collect::<Vec<_>>()),
            move |(repo, rows)| repo_group(state, review, repo, rows, on_open.clone()),
        )
        .style(|s| s.flex_col().width_full().padding_bottom(10.0))
        .scroll()
        .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
        Empty::new().style(|s| s.height(1.0).width_full().background(theme::BORDER_MUTED)),
        Empty::new().style(|s| s.height(6.0).width_full().flex_shrink(0.0_f32)),
        segment_row(state, review, WorktreeSegment::Archived),
        Empty::new().style(|s| s.height(6.0).width_full().flex_shrink(0.0_f32)),
    ))
    .style(|s| s.size_full().flex_col())
}

fn repo_group(
    state: AppState,
    review: ReviewState,
    repo: String,
    rows: Vec<Worktree>,
    on_open: OpenWorktree,
) -> impl IntoView {
    // The owner is in every row's branch and in the card on the screen
    // beside this; the heading only needs the name.
    let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();

    Stack::vertical((
        Label::derived(move || name.clone()).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_SUBTLE)
                .padding_horiz(16.0)
                .padding_top(14.0)
                .padding_bottom(5.0)
                .text_ellipsis()
                .selectable(false)
        }),
        dyn_stack(
            move || rows.clone(),
            |w| w.id.clone(),
            move |w| worktree_row(state, review, w, on_open.clone()),
        )
        .style(|s| s.flex_col().width_full().gap(1.0)),
    ))
    .style(|s| s.flex_col().width_full())
}

/// One checkout: a dot in the pull request's colour, and its title.
fn worktree_row(
    state: AppState,
    review: ReviewState,
    worktree: Worktree,
    on_open: OpenWorktree,
) -> impl IntoView {
    use floem::peniko::Color;
    use floem::text::FontWeight;

    // One closure per use: each captures a String, so none of them is Copy.
    let (id_a, id_b, id_c, id_d) =
        (worktree.id.clone(), worktree.id.clone(), worktree.id.clone(), worktree.id.clone());
    let is = |id: String| move || review.worktree.with(|w| w.as_ref().is_some_and(|w| w.id == id));
    let (selected_dot, selected_text, selected_click, selected_box) =
        (is(id_a), is(id_b), is(id_c), is(id_d));
    let title = worktree
        .pull_request
        .as_ref()
        .map(|pr| pr.title.clone())
        .unwrap_or_else(|| worktree.branch.clone());
    let color = crate::ui::worktrees::state_color_of(&worktree);
    let open_target = worktree.clone();

    let content = Stack::horizontal((
        Empty::new().style(move |s| {
            let base = s.size(8.0, 8.0).border_radius(4.0).flex_shrink(0.0_f32);
            // Filled while it is the one being read; a ring otherwise.
            if selected_dot() {
                base.background(color)
            } else {
                base.border(1.5).border_color(color)
            }
        }),
        Label::derived(move || title.clone()).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(if selected_text() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                .color(if selected_text() { theme::FG } else { theme::FG_MUTED })
                .margin_left(10.0)
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
        }),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0));

    Container::new(content)
        .on_event_stop(el::Click, move |_, _| {
            if !selected_click() {
                state.show_screen(review, Screen::Worktrees);
                on_open(&open_target);
            }
        })
        .style(move |s| {
            let base = crate::ui::widgets::clickable(s)
                .width_full()
                .height(30.0)
                .items_center()
                .padding_horiz(10.0)
                .margin_horiz(6.0)
                .border_radius(theme::RADIUS);
            if selected_box() {
                base.background(theme::CANVAS).border(1.0).border_color(theme::BORDER)
            } else {
                base.border(1.0)
                    .border_color(Color::TRANSPARENT)
                    .hover(|s| s.background(theme::CANVAS_HOVER))
            }
        })
}

/// Active or Archived, in the worktrees panel. Same card as a view row, so
/// the two sections of the app read as one control.
fn segment_row(state: AppState, review: ReviewState, segment: WorktreeSegment) -> impl IntoView {
    use floem::peniko::Color;
    use floem::text::FontWeight;

    let selected = move || state.worktree_segment.get() == segment && !review.is_open();
    let count = move || state.worktree_count(segment);
    let icon = match segment {
        WorktreeSegment::Active => SidebarIcon::Branch,
        WorktreeSegment::Archived => SidebarIcon::Archive,
    };

    let content = Stack::horizontal((
        dyn_container(
            move || if selected() { theme::FG } else { theme::FG_MUTED },
            move |c| crate::ui::widgets::sidebar_glyph(icon, c).into_any(),
        )
        .style(|s| s.size(16.0, 16.0)),
        Label::derived(move || segment.label()).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(if selected() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                .color(if selected() { theme::FG } else { theme::FG_MUTED })
                .margin_left(9.0)
                .flex_grow(1.0_f32)
        }),
        dyn_container(count, move |n| {
            if n == 0 {
                Empty::new().into_any()
            } else {
                count_badge(move || n).into_any()
            }
        }),
    ))
    .style(|s| s.items_center().width_full());

    Container::new(content)
        .on_event_stop(el::Click, move |_, _| {
            state.show_screen(review, Screen::Worktrees);
            state.worktree_segment.set(segment);
        })
        .style(move |s| {
            let base = crate::ui::widgets::clickable(s)
                .width_full()
                .height(34.0)
                .items_center()
                .padding_horiz(10.0)
                .margin_horiz(6.0)
                .border_radius(theme::RADIUS);
            if selected() {
                base.background(theme::CANVAS).border(1.0).border_color(theme::BORDER)
            } else {
                base.border(1.0)
                    .border_color(Color::TRANSPARENT)
                    .hover(|s| s.background(theme::CANVAS_HOVER))
            }
        })
}

fn filters_section(state: AppState) -> impl IntoView {
    dyn_stack(
        || ReasonFilter::ALL,
        |f| *f,
        move |f| {
            let selected = move || state.reason_filter.get() == Some(f);
            let count = move || state.counts().by_reason.get(&f).copied().unwrap_or(0);
            Container::new(sidebar_row(
                icon_for(f),
                f.label().to_string(),
                selected,
                true,
                Some(Box::new(count)),
            ))
            .on_event_stop(el::Click, move |_, _| state.toggle_reason(f))
        },
    )
    .style(|s| s.flex_col().width_full().gap(1.0))
}

fn icon_for(f: ReasonFilter) -> SidebarIcon {
    match f {
        ReasonFilter::Assigned => SidebarIcon::Target,
        ReasonFilter::Participating => SidebarIcon::Comment,
        ReasonFilter::Mentioned => SidebarIcon::Mention,
        ReasonFilter::TeamMentioned => SidebarIcon::People,
        ReasonFilter::ReviewRequested => SidebarIcon::Eye,
    }
}

fn repos_section(state: AppState) -> impl IntoView {
    dyn_stack(
        move || state.counts().repos,
        // Keyed on the count as well as the name: the row draws the number it
        // was built with, so a repository whose count moved has to be rebuilt
        // or it keeps the old one.
        |(name, count, _)| (name.clone(), *count),
        move |(name, count, avatar_url)| {
            let click_name = name.clone();
            Container::new(repo_row(state, name, count, avatar_url))
                .on_event_stop(el::Click, move |_, _| state.toggle_repo(&click_name))
        },
    )
    .style(|s| s.flex_col().width_full().gap(1.0).padding_bottom(10.0))
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

fn repo_row(state: AppState, name: String, count: usize, avatar_url: String) -> impl IntoView {
    use floem::peniko::Color;
    use floem::text::FontWeight;
    use floem::views::{Label, Stack};

    // Owner avatars make a long repository list scannable at a glance.
    let seed = name.clone();
    let label_text = name.clone();

    // One clone per closure: the capture is a String, so these cannot share a
    // single `Copy` predicate the way the reason filters do.
    let (sel_a, sel_b, sel_c) = (name.clone(), name.clone(), name.clone());
    let selected_text = move || state.repo_filter.with(|r| r.as_deref() == Some(sel_a.as_str()));
    let selected_weight = move || state.repo_filter.with(|r| r.as_deref() == Some(sel_b.as_str()));
    let selected_box = move || state.repo_filter.with(|r| r.as_deref() == Some(sel_c.as_str()));

    let content = Stack::horizontal((
        avatar(state, move || avatar_url.clone(), 16.0, seed).style(|s| s.margin_right(9.0)),
        Label::derived(move || label_text.clone()).style(move |s| {
            s.font_size(theme::TEXT_BODY)
                .color(if selected_text() { theme::FG } else { theme::FG_MUTED })
                .font_weight(if selected_weight() {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_ellipsis()
                .flex_grow(1.0_f32)
                .min_width(0.0)
        }),
        count_badge(move || count),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0));

    Container::new(content).style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .width_full()
            .height(34.0)
            .items_center()
            .padding_horiz(10.0)
            .margin_horiz(6.0)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::CANVAS_HOVER).cursor(floem::style::CursorStyle::Pointer));
        if selected_box() {
            base.background(theme::ACCENT_SUBTLE)
                .border(1.0)
                .border_color(theme::ACCENT_RING)
        } else {
            base.border(1.0).border_color(Color::TRANSPARENT)
        }
    })
}


/// The activity rail.
///
/// Modelled on an editor's: a narrow column of destinations that is always
/// on screen, with the current one marked by a stripe down its left edge.
/// Clicking the entry that is already showing hides the panel beside it, and
/// clicking it again brings the panel back. There is no separate collapse
/// button, because the rail itself is the control.
///
/// It is wider than the 48 pixels an editor uses because the traffic lights
/// sit at the top of it and need the room. Since the rail is never hidden,
/// nothing else in the window has to leave space for them.
pub fn activity_rail(state: AppState, review: ReviewState) -> impl IntoView {
    use floem::views::drag_window_area;

    Stack::vertical((
        // The lights sit over this.
        drag_window_area(Empty::new().style(|s| s.size_full()))
            .style(|s| s.height(theme::TITLEBAR_HEIGHT).width_full().flex_shrink(0.0_f32)),
        rail_entry(state, review, RailSection::Notifications),
        rail_entry(state, review, RailSection::Worktrees),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
    ))
    .style(|s| {
        s.width(theme::ACTIVITY_RAIL_WIDTH)
            .height_full()
            .flex_col()
            .flex_shrink(0.0_f32)
            .items_center()
            .background(theme::CANVAS_RAIL)
            .border_right(1.0)
            .border_color(theme::BORDER)
    })
}

/// The two sections of the app, as the rail names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailSection {
    /// Inbox, Saved and Done, chosen in the panel beside the rail.
    Notifications,
    /// The checkouts, active and archived. A review is one of these being
    /// read, so the entry stays current while one is open.
    Worktrees,
}

/// One rail entry: a glyph, a stripe while its section is on screen, and
/// for notifications the unread count. Clicking the current entry folds the
/// panel away; clicking the other switches to it with the panel showing.
fn rail_entry(state: AppState, review: ReviewState, section: RailSection) -> impl IntoView {
    use floem::peniko::Color;
    use floem::style::Position;

    let unread = move || match section {
        RailSection::Notifications => state.counts().unread,
        // A count of worktrees is not something to act on.
        RailSection::Worktrees => 0,
    };
    let active = move || match section {
        RailSection::Notifications => {
            !review.is_showing() && state.screen.get() == Screen::Threads
        }
        RailSection::Worktrees => review.is_showing() || state.screen.get() == Screen::Worktrees,
    };
    let glyph = match section {
        RailSection::Notifications => SidebarIcon::Bell,
        RailSection::Worktrees => SidebarIcon::Branch,
    };

    Container::new(
        Container::new(Stack::horizontal((
            dyn_container(active, move |on| {
                crate::ui::widgets::sidebar_glyph(
                    glyph,
                    if on { theme::FG } else { theme::FG_SUBTLE },
                )
                .style(|s| s.size(20.0, 20.0))
                .into_any()
            })
            .style(|s| s.size(20.0, 20.0)),
            unread_badge(unread),
        )))
        .style(move |s| {
            let base = crate::ui::widgets::clickable(s)
                .size(theme::RAIL_ENTRY_SIZE, theme::RAIL_ENTRY_SIZE)
                .items_center()
                .justify_center()
                .border_radius(theme::RADIUS)
                .position(Position::Relative);
            if active() {
                base.background(theme::CANVAS)
            } else {
                base.hover(|s| s.background(theme::CANVAS_HOVER))
            }
        }),
    )
    .on_event_stop(el::Click, move |_, _| {
        if active() && !review.is_showing() {
            // Already here, so this is the collapse control.
            state.sidebar_open.update(|open| *open = !*open);
        } else {
            // Over from the other section, or back from a pull request,
            // with the panel showing so the section's lists are in reach.
            let screen = match section {
                RailSection::Notifications => Screen::Threads,
                RailSection::Worktrees => Screen::Worktrees,
            };
            state.show_screen(review, screen);
            state.sidebar_open.set(true);
        }
    })
    .style(move |s| {
        // The stripe is the container's own left border, so it runs the full
        // height of the entry rather than floating beside it.
        let base = s
            .width_full()
            .height(theme::RAIL_ENTRY_SIZE)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center();
        // The placeholder is the same width as the stripe, so toggling the
        // entry never shifts its icon sideways.
        if active() {
            base.border_left(theme::RAIL_STRIPE).border_color(theme::ACCENT)
        } else {
            base.border_left(theme::RAIL_STRIPE).border_color(Color::TRANSPARENT)
        }
    })
}

/// A small count over the corner of a rail icon.
fn unread_badge(unread: impl Fn() -> usize + 'static + Copy) -> impl IntoView {
    use floem::style::Position;

    dyn_container(
        unread,
        move |n| {
            if n == 0 {
                Empty::new().into_any()
            } else {
                Container::new(
                    Label::derived(move || if n > 99 { "99+".to_string() } else { n.to_string() })
                        .style(|s| {
                            s.font_size(9.0)
                                .color(theme::CANVAS)
                                .selectable(false)
                                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
                        }),
                )
                .style(|s| {
                    s.position(Position::Absolute)
                        .inset_right(-4.0)
                        .inset_bottom(-3.0)
                        .height(14.0)
                        .min_width(14.0)
                        .padding_horiz(3.0)
                        .border_radius(7.0)
                        .background(theme::ACCENT)
                        .items_center()
                        .justify_center()
                })
                .into_any()
            }
        },
    )
}
