//! The drawer on the left, which is the whole navigation.
//!
//! Top to bottom: a strip the traffic lights sit over, the search box and
//! the filter button, the bell that opens the inbox, then Priority, Pinned,
//! and one section per project. While the inbox is open the drawer shows
//! that screen's own views, filters and repositories instead, with a row
//! at the top that comes back here.

use std::rc::Rc;

use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    Container, Decorators, Empty, Label, ScrollExt, Stack, TextInput, dyn_container, dyn_stack,
};

use crate::backend::Backend;
use perch_github::Item;
use crate::filter::{ReasonFilter, View};
use crate::icons::SidebarIcon;
use crate::items::{ItemGroup, ItemSort, ItemStatus};
use crate::review::model::ReviewState;
use crate::state::{AppState, Screen};
use crate::theme;
use crate::ui::items::{item_row, section_header, OpenItem, Placement};
use crate::ui::titlebar::title_bar;
use crate::ui::widgets::{avatar, count_badge, section_heading, sidebar_glyph, sidebar_row};

/// The drawer, open.
pub fn sidebar(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    on_open: OpenItem,
) -> impl IntoView {
    Stack::vertical((
        lights_strip(state),
        dyn_container(
            move || !review.is_showing() && state.screen.get() == Screen::Inbox,
            move |inbox| {
                if inbox {
                    inbox_nav(state, review).into_any()
                } else {
                    projects_nav(state, review, backend.clone(), on_open.clone()).into_any()
                }
            },
        )
        .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0).flex_col()),
    ))
    .style(move |s| {
        s.width(state.sidebar_width.get())
            .height_full()
            .flex_shrink(0.0_f32)
            .flex_col()
            .background(theme::CANVAS_SIDEBAR)
            .border_right(1.0)
            .border_color(theme::BORDER)
    })
}

/// The drawer, folded: the strip the lights sit over and the way back.
pub fn collapsed_sidebar(state: AppState) -> impl IntoView {
    Stack::vertical((
        title_bar(Empty::new()).style(|s| s.height(theme::TITLEBAR_HEIGHT).width_full().flex_shrink(0.0_f32)),
        toggle_button(state).style(|s| s.margin_top(2.0)),
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

/// The strip the traffic lights sit over, with the drawer's toggle at its
/// right edge.
fn lights_strip(state: AppState) -> impl IntoView {
    title_bar(
        Stack::horizontal((
            Empty::new().style(|s| s.flex_grow(1.0_f32)),
            toggle_button(state),
        ))
        .style(|s| s.items_center().width_full().padding_right(8.0)),
    )
    .style(|s| s.height(theme::TITLEBAR_HEIGHT).width_full().flex_shrink(0.0_f32).items_center())
}

/// Show or hide the drawer.
fn toggle_button(state: AppState) -> impl IntoView {
    let open = move || state.sidebar_open.get();
    Container::new(dyn_container(open, |open| {
        sidebar_glyph(
            if open { SidebarIcon::ChevronLeft } else { SidebarIcon::ChevronRight },
            theme::FG_MUTED,
        )
        .style(|s| s.size(14.0, 14.0))
        .into_any()
    }))
    .on_event_stop(el::Click, move |_, _| state.sidebar_open.update(|o| *o = !*o))
    .style(|s| {
        crate::ui::widgets::clickable(s)
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
}

// ---------------------------------------------------------------------
// The projects navigation
// ---------------------------------------------------------------------

fn projects_nav(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    on_open: OpenItem,
) -> impl IntoView {
    Stack::vertical((
        search_row(state, backend.clone()),
        bell_row(state, review),
        sections(state, backend, on_open),
    ))
    .style(|s| s.size_full().flex_col())
}

/// The search box and the filter button.
fn search_row(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let input = TextInput::new(state.item_query)
        .placeholder("Search")
        .on_event_cont(el::FocusGained, move |_, _| state.search_focused.set(true))
        .on_event_cont(el::FocusLost, move |_, _| state.search_focused.set(false))
        .style(|s| {
            s.flex_grow(1.0_f32)
                .min_width(0.0)
                .height(28.0)
                .font_size(theme::TEXT_META)
                .padding_horiz(8.0)
                .background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::ACCENT))
        });
    state.search_box.set(Some(input.view_id()));

    Stack::horizontal((input, filter_button(state, backend)))
        .style(|s| s.items_center().width_full().gap(6.0).padding_horiz(12.0).padding_vert(6.0))
}

/// The filter menu, in the shape of a settings menu: one row per setting,
/// each a submenu with its options checked.
fn filter_button(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let narrowed = move || {
        state.item_filters.with(|f| {
            let d = crate::items::ItemFilters::default();
            f.status != d.status || f.kinds != d.kinds || f.group != d.group || f.sort != d.sort || f.show_empty
        })
    };
    Container::new(dyn_container(narrowed, |on| {
        sidebar_glyph(SidebarIcon::Filter, if on { theme::ACCENT } else { theme::FG_MUTED })
            .style(|s| s.size(14.0, 14.0).pointer_events_none())
            .into_any()
    }))
    .popout_menu(move || filter_menu(state, backend.clone()))
    .style(|s| {
        crate::ui::widgets::clickable(s)
            .size(28.0, 28.0)
            .items_center()
            .justify_center()
            .background(theme::CANVAS)
            .border(1.0)
            .border_color(theme::BORDER)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
}

fn filter_menu(state: AppState, backend: Rc<Backend>) -> Menu {
    let f = state.item_filters.get_untracked();
    // One writer, so every change lands in the meta table the same way.
    let set = Rc::new(move |edit: Box<dyn Fn(&mut crate::items::ItemFilters)>| {
        state.item_filters.update(|f| edit(f));
        backend.set_meta("item_filters", &state.item_filters.get_untracked().to_meta());
    });

    let mut menu = Menu::new();
    menu = menu.submenu("Status", {
        let set = set.clone();
        let current = f.status;
        move |mut sub| {
            for status in ItemStatus::ALL {
                let set = set.clone();
                sub = sub.item(status.label(), move |m| {
                    let set = set.clone();
                    m.checked(status == current).action(move || set(Box::new(move |f| f.status = status)))
                });
            }
            sub
        }
    });
    menu = menu.submenu("Kind", {
        let set = set.clone();
        let kinds = f.kinds;
        move |sub| {
            let (a, b, c) = (set.clone(), set.clone(), set.clone());
            sub.item("Pull Requests", move |m| {
                let a = a.clone();
                m.checked(kinds.pull_requests).action(move || a(Box::new(|f| f.kinds.pull_requests = !f.kinds.pull_requests)))
            })
            .item("Issues", move |m| {
                let b = b.clone();
                m.checked(kinds.issues).action(move || b(Box::new(|f| f.kinds.issues = !f.kinds.issues)))
            })
            .item("Branches", move |m| {
                let c = c.clone();
                m.checked(kinds.branches).action(move || c(Box::new(|f| f.kinds.branches = !f.kinds.branches)))
            })
        }
    });
    menu = menu.submenu("Group By", {
        let set = set.clone();
        let current = f.group;
        move |mut sub| {
            for group in ItemGroup::ALL {
                let set = set.clone();
                sub = sub.item(group.label(), move |m| {
                    let set = set.clone();
                    m.checked(group == current).action(move || set(Box::new(move |f| f.group = group)))
                });
            }
            sub
        }
    });
    menu = menu.submenu("Sort By", {
        let set = set.clone();
        let current = f.sort;
        move |mut sub| {
            for sort in ItemSort::ALL {
                let set = set.clone();
                sub = sub.item(sort.label(), move |m| {
                    let set = set.clone();
                    m.checked(sort == current).action(move || set(Box::new(move |f| f.sort = sort)))
                });
            }
            sub
        }
    });
    menu = menu.separator();
    {
        let set = set.clone();
        menu = menu.item("Show Empty Projects", move |m| {
            let set = set.clone();
            m.checked(f.show_empty).action(move || set(Box::new(|f| f.show_empty = !f.show_empty)))
        });
    }
    {
        let set = set.clone();
        menu = menu.item("Show PR Status", move |m| {
            let set = set.clone();
            m.checked(f.show_pr_status).action(move || set(Box::new(|f| f.show_pr_status = !f.show_pr_status)))
        });
    }
    menu
}

/// The bell: the inbox, with how many unread notifications are in it.
fn bell_row(state: AppState, review: ReviewState) -> impl IntoView {
    let unread = move || state.counts().unread;
    Container::new(
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Bell, theme::FG_MUTED).style(|s| s.size(16.0, 16.0)),
            Label::derived(|| "Inbox".to_string()).style(|s| {
                s.font_size(theme::TEXT_BODY).color(theme::FG).margin_left(9.0).flex_grow(1.0_f32).selectable(false)
            }),
            dyn_container(unread, move |n| {
                if n == 0 {
                    Empty::new().into_any()
                } else {
                    count_badge(move || n).into_any()
                }
            }),
        ))
        .style(|s| s.items_center().width_full()),
    )
    .on_event_stop(el::Click, move |_, _| state.show_screen(review, Screen::Inbox))
    .style(|s| {
        crate::ui::widgets::clickable(s)
            .width_full()
            .height(34.0)
            .items_center()
            .padding_horiz(10.0)
            .margin_horiz(6.0)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::CANVAS_HOVER))
    })
}

/// Priority, Pinned, and the groups, in one scrolling column.
fn sections(state: AppState, backend: Rc<Backend>, on_open: OpenItem) -> impl IntoView {
    let (b1, b2, b3) = (backend.clone(), backend.clone(), backend);
    let (o1, o2, o3) = (on_open.clone(), on_open.clone(), on_open);

    Stack::vertical((
        // Priority
        Stack::vertical((
            section_header("Priority".into(), 0, None),
            dyn_stack(
                move || state.item_sections().priority,
                row_key,
                move |item| {
                    let project = state.project(&item.project_id);
                    item_row(state, b1.clone(), item, project, Placement::Priority, o1.clone())
                },
            )
            .style(|s| s.flex_col().width_full().gap(1.0)),
            dyn_container(
                move || state.item_sections().priority.is_empty(),
                |empty| {
                    if empty {
                        quiet_line("Nothing needs attention").into_any()
                    } else {
                        Empty::new().into_any()
                    }
                },
            ),
        ))
        .style(|s| s.flex_col().width_full()),
        // Pinned, hidden when empty.
        dyn_container(
            move || state.item_sections().pinned.is_empty(),
            move |empty| {
                if empty {
                    return Empty::new().into_any();
                }
                let (b, o) = (b2.clone(), o2.clone());
                Stack::vertical((
                    section_header("Pinned".into(), 0, None),
                    dyn_stack(
                        move || state.item_sections().pinned,
                        row_key,
                        move |item| {
                            let project = state.project(&item.project_id);
                            item_row(state, b.clone(), item, project, Placement::Pinned, o.clone())
                        },
                    )
                    .style(|s| s.flex_col().width_full().gap(1.0)),
                ))
                .style(|s| s.flex_col().width_full())
                .into_any()
            },
        ),
        // One section per project, or per date.
        dyn_stack(
            move || state.item_sections().groups,
            |section| (section.key.clone(), section.items.iter().map(row_key).collect::<Vec<_>>(), section.count),
            move |section| group_section(state, b3.clone(), section, o3.clone()),
        )
        .style(|s| s.flex_col().width_full().padding_bottom(12.0)),
    ))
    .style(|s| s.flex_col().width_full())
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// What a row is rebuilt on: everything it draws.
type RowKey = (String, Option<jiff::Timestamp>, Option<jiff::Timestamp>, bool, Option<String>, String, bool, bool, Option<String>);

fn row_key(item: &Item) -> RowKey {
    (
        item.id.clone(),
        item.attention_at,
        item.done_at,
        item.saved,
        item.name.clone(),
        item.given_title().to_string(),
        item.is_pinned(),
        item.is_archived(),
        item.checkout.as_ref().map(|c| format!("{}:{}", c.branch, c.on_disk)),
    )
}

fn group_section(
    state: AppState,
    backend: Rc<Backend>,
    section: crate::items::ItemSection,
    on_open: OpenItem,
) -> impl IntoView {
    let (key_a, key_b) = (section.key.clone(), section.key.clone());
    let toggle_key = section.key.clone();
    let by_date = section.key.starts_with("date:");
    let project_id = section.key.strip_prefix("project:").map(str::to_string);
    let collapsed = move || state.collapsed.with(|c| c.contains(&key_a));
    let collapsed_body = move || state.collapsed.with(|c| c.contains(&key_b));
    let items = section.items.clone();
    let title = section.title.clone();
    let count = section.count;
    let placement = Placement::Section(if by_date { ItemGroup::Date } else { ItemGroup::Project });

    Stack::vertical((
        dyn_container(collapsed, move |c| section_header(title.clone(), count, Some(c)).into_any())
            .on_event_stop(el::Click, move |_, _| {
                state.collapsed.update(|c| {
                    if !c.remove(&toggle_key) {
                        c.insert(toggle_key.clone());
                    }
                });
                let joined: Vec<String> = state.collapsed.get_untracked().into_iter().collect();
                state.pending_meta.set(Some(("collapsed_projects".to_string(), joined.join(","))));
            })
            .style(|s| s.width_full()),
        dyn_container(collapsed_body, move |c| {
            if c {
                return Empty::new().into_any();
            }
            let (b, o, items) = (backend.clone(), on_open.clone(), items.clone());
            let project_id = project_id.clone();
            dyn_stack(
                move || items.clone(),
                row_key,
                move |item| {
                    let project = project_id
                        .as_deref()
                        .or(Some(item.project_id.as_str()))
                        .and_then(|id| state.project(id));
                    item_row(state, b.clone(), item, project, placement, o.clone())
                },
            )
            .style(|s| s.flex_col().width_full().gap(1.0))
            .into_any()
        }),
    ))
    .style(|s| s.flex_col().width_full())
}

fn quiet_line(text: &'static str) -> impl IntoView {
    Label::derived(move || text.to_string()).style(|s| {
        s.font_size(theme::TEXT_META)
            .color(theme::FG_SUBTLE)
            .padding_horiz(16.0)
            .padding_vert(6.0)
            .selectable(false)
    })
}

// ---------------------------------------------------------------------
// The inbox navigation
// ---------------------------------------------------------------------

/// The notifications screen's own drawer: a way back, then its views,
/// filters and repositories as they were.
fn inbox_nav(state: AppState, review: ReviewState) -> impl IntoView {
    Stack::vertical((
        back_row(state, review),
        views_section(state, review),
        section_heading("Filters"),
        filters_section(state),
        section_heading("Repositories"),
        repos_section(state),
    ))
    .style(|s| s.size_full().flex_col())
}

fn back_row(state: AppState, review: ReviewState) -> impl IntoView {
    Container::new(
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::ChevronLeft, theme::FG_MUTED).style(|s| s.size(12.0, 12.0)),
            Label::derived(|| "Projects".to_string()).style(|s| {
                s.font_size(theme::TEXT_META).color(theme::FG_MUTED).margin_left(6.0).selectable(false)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .on_event_stop(el::Click, move |_, _| state.show_screen(review, Screen::Sidebar))
    .style(|s| {
        crate::ui::widgets::clickable(s)
            .height(30.0)
            .items_center()
            .padding_horiz(10.0)
            .margin_horiz(6.0)
            .margin_top(2.0)
            .margin_bottom(4.0)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::CANVAS_HOVER))
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
            state.show_screen(review, Screen::Inbox);
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
                base.background(theme::CANVAS_SELECTED).border(1.0).border_color(Color::TRANSPARENT)
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


/// The strip along the panel's right edge that drags it wider or narrower.
///
/// Sits over the panel's border rather than beside it, so the layout does
/// not gain a gap. The new width is the pointer's distance from the panel's
/// left edge, which is the rail's right edge; no delta bookkeeping, so a
/// fast drag cannot drift.
pub fn resize_handle(state: AppState) -> impl IntoView {
    use floem::event::DragConfig;
    use floem::style::{CursorStyle, Position};

    Empty::new()
        .on_event_stop(el::PointerDown, |cx, e: &PointerButtonEvent| {
            if let Some(pointer) = e.pointer.pointer_id {
                if pointer.is_primary_pointer() {
                    cx.request_pointer_capture(pointer);
                }
            }
        })
        .on_event_stop(el::GainedPointerCapture, |cx, token| {
            // No ghost and no snap-back animation: this is a divider, not
            // a card being carried somewhere.
            cx.start_drag(
                *token,
                DragConfig {
                    threshold: 1.0,
                    animation_duration: std::time::Duration::ZERO,
                    easing: std::rc::Rc::new(floem::easing::Linear),
                    custom_data: None,
                    track_targets: false,
                },
                false,
            );
        })
        .on_event_stop(el::DragStart, move |_, _: &floem::event::DragStartEvent| {
            state.sidebar_resizing.set(true);
        })
        .on_event_stop(el::DragMove, move |cx, e: &floem::event::DragMoveEvent| {
            // The event arrives in the handle's own coordinates, and the
            // handle moves with the edge it drags, so measured from there
            // the pointer never gets far. Map it back to the window first.
            let point = cx.world_transform.inverse() * e.current_state.logical_point();
            let width = (point.x - theme::ACTIVITY_RAIL_WIDTH)
                .clamp(theme::SIDEBAR_MIN_WIDTH, theme::SIDEBAR_MAX_WIDTH);
            state.sidebar_width.set(width);
        })
        .on_event_stop(el::DragEnd, move |_, _: &floem::event::DragEndEvent| {
            state.sidebar_resizing.set(false);
        })
        .on_event_stop(el::DragCancel, move |_, _: &floem::event::DragCancelEvent| {
            state.sidebar_resizing.set(false);
        })
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(0.0)
                .inset_bottom(0.0)
                .inset_right(-3.0)
                .width(7.0)
                .z_index(10)
                .hover(|s| s.cursor(CursorStyle::ColResize))
        })
}

/// A sheet over the whole window while the handle is being dragged, so the
/// cursor stays the column-resize one.
///
/// The cursor belongs to whatever is under the pointer, and the handle is
/// only there once layout has caught up with the last move. On a fast drag,
/// or past either width limit, the pointer is over a row instead, and the
/// cursor becomes the row's. Pointer events still go to the handle, which
/// holds capture; the sheet only takes the hover.
pub fn resize_cursor_sheet(state: AppState) -> impl IntoView {
    use floem::style::{CursorStyle, Position};

    Empty::new().style(move |s| {
        s.position(Position::Absolute)
            .inset(0.0)
            .z_index(100)
            .cursor(CursorStyle::ColResize)
            .apply_if(!state.sidebar_resizing.get(), |s| s.hide())
    })
}

