//! The drawer on the left, which is the whole navigation.
//!
//! Top to bottom: the title strip with Inbox / Worktrees / Browse, search
//! and filters, then the tab's list.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    dyn_container, dyn_stack, Button, Container, Decorators, Empty, Label, ScrollExt, Stack, TextInput,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::items::{
    BrowseView, Direction, InboxView, ItemFilters, ItemSort, Order, SidebarTab, StateFilter, WorktreeView,
};
use crate::state::AppState;
use crate::theme;
use crate::ui::items::{item_row, OpenItem};
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::titlebar::title_bar;
use crate::ui::widgets::sidebar_glyph;
use crate::update::UpdateStatus;
use perch_github::Item;

/// The drawer, open.
pub fn sidebar(state: AppState, backend: Rc<Backend>, on_open: OpenItem) -> impl IntoView {
    Stack::vertical((
        lights_strip(state, backend.clone()),
        items_nav(state, backend.clone(), on_open)
            .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0).flex_col()),
        footer(state, backend.clone()),
    ))
    .style(move |s| {
        s.width(state.sidebar_width.get())
            .height_full()
            .flex_shrink(0.0_f32)
            .flex_col()
            .background(theme::nav_canvas())
            .border_right(1.0)
            .border_color(theme::nav_divider())
    })
}

/// The strip the traffic lights sit over, followed by the drawer control,
/// the three tabs, and `+` for new work.
fn lights_strip(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    title_bar(
        Stack::horizontal((
            lights_lane(state, 0.0),
            scope_tabs(state).style(|s| s.margin_left(8.0)),
            Empty::new().style(|s| s.flex_grow(1.0_f32).pointer_events_none()),
            new_work_button(state, backend),
        ))
        .debug_name("drawer strip")
        .style(|s| s.items_center().width_full().padding_right(12.0)),
    )
    .style(|s| s.height(theme::TITLEBAR_HEIGHT).width_full().flex_shrink(0.0_f32).items_center())
}

/// The lane the native traffic lights occupy, then the drawer control right
/// after them, as Codex has it.
///
/// The drawer's strip starts with this while the drawer is open. While it is
/// shut there is no drawer, and the bar over the main pane starts with it
/// instead, so the lights and the way back are always at the top left.
///
/// The lane is measured from the window's edge; `inset` is how far in the
/// bar's own padding has already put its first child.
///
/// The lane is part of whichever bar it is in, and lets a press through to
/// it, so the window drags from beside the lights; the control alone
/// takes the pointer.
pub fn lights_lane(state: AppState, inset: f64) -> impl IntoView {
    Stack::horizontal((
        Empty::new().style(move |s| s.width(theme::ACTIVITY_RAIL_WIDTH - inset)),
        toggle_button(state),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32).pointer_events_none())
}

/// `+`: the new-work view, as `Cmd-N` opens it.
fn new_work_button(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    Container::new(
        sidebar_glyph(SidebarIcon::Plus, theme::nav_muted).style(|s| s.size(16.0, 16.0)),
    )
    .on_event_stop(el::Click, move |_, _| crate::ui::new_work::open(state, &backend))
    .debug_name("new work")
    .style(move |s| {
        let open = state.new_work.open.get();
        crate::ui::widgets::clickable(s)
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(8.0)
            .pointer_events_auto()
            .apply_if(open, |s| s.background(theme::nav_selected()))
            .hover(|s| s.background(theme::nav_hover()))
    })
}

/// Show or hide the drawer. The same glyph either way, as the system's
/// sidebar toggle is: it names the drawer, not the direction.
fn toggle_button(state: AppState) -> impl IntoView {
    Container::new(
        sidebar_glyph(SidebarIcon::Drawer, theme::nav_muted).style(|s| s.size(16.0, 16.0)),
    )
    .on_event_stop(el::Click, move |_, _| state.sidebar_open.update(|o| *o = !*o))
    .debug_name("drawer toggle")
    .style(|s| {
        crate::ui::widgets::clickable(s)
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(8.0)
            .pointer_events_auto()
            .hover(|s| s.background(theme::nav_hover()))
    })
}

/// The strip along the drawer's foot, as tall as the bar over the main
/// pane: who is signed in, or the restart for an update that's waiting,
/// then what belongs to the app rather than to an item, invites, bug
/// reports and plan usage.
fn footer(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let login = move || state.viewer.with(|v| v.as_ref().map(|(l, _)| l.clone()).unwrap_or_default());
    let url = move || state.viewer.with(|v| v.as_ref().map(|(_, a)| a.clone()).unwrap_or_default());
    // The face is built once, and the viewer can arrive after it: the
    // initial follows the login.
    let face = dyn_container(login, move |login| {
        crate::ui::widgets::avatar(state, url, 22.0, login).into_any()
    })
    .style(|s| s.flex_shrink(0.0_f32));
    let controls = match state.review.get_untracked() {
        Some(review) => Stack::horizontal((
            crate::ui::invite::button(state, review, backend.clone()),
            crate::ui::feedback::button(state, review, backend.clone()),
            crate::ui::usage::button(state, review, backend.clone()),
        ))
        .into_any(),
        None => Empty::new().into_any(),
    };
    // An update waiting for a restart takes the login's place.
    let updating = floem::reactive::Memo::new(move |_| state.update.with(|u| *u != UpdateStatus::Current));
    let middle = {
        let backend = backend.clone();
        dyn_container(
            move || updating.get(),
            move |updating| {
                if updating {
                    crate::ui::update::button(state, backend.clone()).into_any()
                } else {
                    Label::derived(login)
                        .style(|s| {
                            s.font_size(theme::TEXT_BODY)
                                .color(theme::nav_fg())
                                .text_ellipsis()
                                .min_width(0.0)
                        })
                        .into_any()
                }
            },
        )
        .style(|s| {
            s.items_center()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .margin_left(8.0)
                .margin_right(8.0)
        })
    };
    Stack::horizontal((
        face,
        middle,
        controls.style(|s| s.items_center().gap(4.0).flex_shrink(0.0_f32)),
    ))
    .debug_name("drawer footer")
    .style(|s| {
        s.items_center()
            .width_full()
            .height(theme::TITLEBAR_HEIGHT)
            .padding_horiz(theme::NAV_GUTTER)
            .flex_shrink(0.0_f32)
            .border_top(1.0)
            .border_color(theme::nav_divider())
    })
}

const FOOTER_BUTTON: f64 = 26.0;

/// A button in the footer, in the drawer's own look, as the drawer
/// control and `+` are, lit while what it opens is up.
pub(crate) fn footer_button(s: floem::style::Style, open: bool) -> floem::style::Style {
    crate::ui::widgets::clickable(s)
        .size(FOOTER_BUTTON, FOOTER_BUTTON)
        .items_center()
        .justify_center()
        .border_radius(8.0)
        .keyboard_navigable()
        .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
        .hover(|s| s.background(theme::nav_hover()))
        .apply_if(open, |s| s.background(theme::nav_selected()))
}

/// Where a footer button's popover hangs: six points over the footer's
/// line, left edges aligned, out over the main pane. The bar's dropdowns
/// hang the same way upside down, under their buttons, right edges
/// aligned.
pub(crate) fn footer_popover(s: floem::style::Style) -> floem::style::Style {
    // From the button's foot: the button, the footer above it, six points.
    let over = FOOTER_BUTTON + (theme::TITLEBAR_HEIGHT - FOOTER_BUTTON) / 2.0 + 6.0;
    s.position(floem::style::Position::Absolute)
        .inset_bottom(over)
        .inset_left(0.0)
        .z_index(100)
}

/// How tall a footer popover can stand, `most` at the tallest: from just
/// over the footer up to the bar's line, so a short window never pushes
/// it over the traffic lights.
pub(crate) fn footer_popover_room(state: AppState, most: f64) -> f64 {
    let height = state.window_height.get();
    if height <= 0.0 {
        return most;
    }
    // The footer and the bar, each as tall as the other, and six points
    // clear of each.
    most.min(height - 2.0 * theme::TITLEBAR_HEIGHT - 12.0).max(160.0)
}

// ---------------------------------------------------------------------
// The item navigation
// ---------------------------------------------------------------------

fn items_nav(state: AppState, backend: Rc<Backend>, on_open: OpenItem) -> impl IntoView {
    Stack::vertical((search_row(state, backend.clone()), item_list(state, backend, on_open)))
        .style(|s| s.size_full().flex_col())
}

/// Inbox, Worktrees and Browse. Selection uses the same persistence path
/// as the filter menu, so changing tabs leaves the other filters intact.
fn scope_tabs(state: AppState) -> impl IntoView {
    dyn_stack(
        || SidebarTab::ALL,
        |tab| tab.label(),
        move |tab| {
            Button::new(Label::new(tab.label()).style(|s| s.pointer_events_none()))
                .action(move || set_filters(state, move |f| f.tab = tab))
                .debug_name(format!("tab:{}", tab.label()))
                .style(move |s| {
                    let selected = state.item_filters.with(|f| f.tab == tab);
                    crate::ui::widgets::clickable(s)
                        .height(31.0)
                        .padding_horiz(9.0)
                        .padding_vert(0.0)
                        .border(0.0)
                        .border_radius(10.0)
                        .font_size(theme::TEXT_META)
                        .color(if selected { theme::nav_fg() } else { theme::nav_muted() })
                        .background(if selected { theme::nav_selected() } else { theme::nav_canvas() })
                        .flex_shrink(0.0_f32)
                        .hover(|s| s.background(theme::nav_hover()))
                        .focus_visible(|s| s.outline(2.0).outline_color(theme::nav_control_border()))
                })
        },
    )
    .style(|s| {
        s.flex_row()
            .items_center()
            .gap(2.0)
            .flex_shrink(0.0_f32)
    })
}

pub(crate) fn set_filters(state: AppState, edit: impl FnOnce(&mut crate::items::ItemFilters)) {
    state.item_filters.update(edit);
    state.pending_meta.set(Some(("item_filters".into(), state.item_filters.get_untracked().to_meta())));
}

/// The search box and the filter button. On Browse, Enter opens what the
/// box holds when Open… would read it.
fn search_row(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    use floem::style::Position;

    let menu_backend = backend.clone();
    let input = TextInput::new(state.item_query)
        .debug_name("sidebar search")
        .on_event_cont(el::FocusGained, move |_, _| state.search_focused.set(true))
        .on_event_cont(el::FocusLost, move |_, _| state.search_focused.set(false))
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            let enter = matches!(key.code, Code::Enter | Code::NumpadEnter) && key.modifiers.is_empty();
            let browsing = state.item_filters.with_untracked(|f| f.tab == SidebarTab::Browse);
            let text = state.item_query.get_untracked();
            let hosts = state.github_hosts.get_untracked();
            if enter && browsing && crate::reference::parse(&text, &hosts).is_some() {
                crate::ui::open_box::open_from_search(state, &backend, &text);
                return EventPropagation::Stop;
            }
            EventPropagation::Continue
        })
        .style(|s| {
            s.width_full()
                .min_width(0.0)
                .height(theme::NAV_CONTROL_HEIGHT)
                .font_size(theme::TEXT_BODY)
                // The glyph takes the left padding, and the text starts at
                // the same optical inset as the item titles below it.
                .padding_left(34.0)
                // Room for the clear button, so text never runs under it.
                .padding_right(32.0)
                .background(theme::nav_canvas())
                .border(1.0)
                .border_color(theme::nav_control_border())
                .border_radius(theme::NAV_CONTROL_RADIUS)
                .focus(|s| s.border_color(theme::nav_muted()))
        });
    let id = input.view_id();
    state.search_box.set(Some(id));
    // Cmd-1 with the drawer shut asked for the keyboard before this box
    // existed; it is taken once the box is in the tree.
    let input = input.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if state.focus_search.get_untracked() {
            state.focus_search.set(false);
            id.request_focus();
        }
    });
    // The glyph is laid over the box rather than beside the text, the way
    // the filter button's is, and lets the pointer through: a click on it
    // is a click in the box.
    let glyph = sidebar_glyph(SidebarIcon::Search, theme::nav_subtle)
        .style(|s| s.size(15.0, 15.0).position(Position::Absolute).inset_left(10.0).pointer_events_none());
    // Shown once there is something to clear. Emptying the box leaves the
    // keyboard in it, ready for the next query.
    let clear = Container::new(
        sidebar_glyph(SidebarIcon::Clear, theme::nav_subtle).style(|s| s.size(15.0, 15.0).pointer_events_none()),
    )
    .on_event_stop(el::Click, move |_, _| {
        state.item_query.set(String::new());
        id.request_focus();
    })
    .debug_name("sidebar search clear")
    .style(move |s| {
        crate::ui::widgets::clickable(s)
            .size(22.0, 22.0)
            .items_center()
            .justify_center()
            .border_radius(11.0)
            .position(Position::Absolute)
            .inset_right(6.0)
            .hover(|s| s.background(theme::nav_hover()))
            .apply_if(state.item_query.with(String::is_empty), |s| s.hide())
    });
    // The placeholder names the list the box searches, which the filter
    // menu picks. The input's own is fixed once it is built, so this one
    // is laid over the box as the glyph is, at the text's inset: the
    // border and the padding.
    let placeholder = Label::derived(move || format!("Search {}", state.item_filters.with(|f| f.view_label())))
        .debug_name("sidebar search placeholder")
        .style(move |s| {
            s.position(Position::Absolute)
                .inset_left(35.0)
                .inset_right(32.0)
                .font_size(theme::TEXT_BODY)
                .color(theme::nav_subtle())
                .text_ellipsis()
                .min_width(0.0)
                .selectable(false)
                .pointer_events_none()
                .apply_if(state.item_query.with(|q| !q.is_empty()), |s| s.hide())
        });
    let search_box = Stack::horizontal((input, placeholder, glyph, clear))
        .style(|s| s.items_center().flex_grow(1.0_f32).min_width(0.0));

    Stack::horizontal((search_box, filter_button(state, menu_backend)))
        .style(|s| s.items_center().width_full().gap(8.0).padding_horiz(theme::NAV_GUTTER).padding_top(16.0))
}

/// The filter menu, in the shape of a settings menu, on every tab: View,
/// then what the list is narrowed to, then Sort By, then what is shown and
/// done. A section is left out only where it cannot apply.
fn filter_button(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    // Anything the current list is narrowed or ordered by lights the
    // button.
    let narrowed = move || state.item_filters.with(ItemFilters::narrowed);
    // The menu is on the button, and a click lands on the deepest view
    // under the pointer, so the glyph and the container that rebuilds it
    // both let the click through.
    Container::new(
        dyn_container(narrowed, |on| {
            sidebar_glyph(SidebarIcon::Filter, move || if on { theme::accent() } else { theme::nav_muted() })
                .style(|s| s.size(14.0, 14.0).pointer_events_none())
                .into_any()
        })
        .style(|s| s.pointer_events_none()),
    )
    .dropdown(move || match state.item_filters.with_untracked(|f| f.tab) {
        SidebarTab::Inbox => inbox_menu(state, backend.clone()),
        SidebarTab::Worktrees => worktree_menu(state),
        SidebarTab::Browse => browse_menu(state),
    })
    .debug_name("sidebar filter")
    .style(move |s| {
        crate::ui::widgets::clickable(s)
            .size(32.0, 32.0)
            .items_center()
            .justify_center()
            .background(if narrowed() { theme::nav_selected_pressed() } else { theme::nav_selected() })
            .border(0.0)
            .border_radius(16.0)
            .hover(|s| s.background(theme::nav_selected_pressed()))
    })
}

/// One writer, so every change lands in the meta table the same way.
type SetFilters = Rc<dyn Fn(Box<dyn FnOnce(&mut ItemFilters)>)>;

fn setter(state: AppState) -> SetFilters {
    Rc::new(move |edit: Box<dyn FnOnce(&mut ItemFilters)>| set_filters(state, edit))
}

/// View: the lists the tab can show, the current one checked.
fn view_submenu<V: Copy + PartialEq + std::fmt::Display + 'static>(
    menu: Menu,
    icon: SidebarIcon,
    views: &[V],
    current: V,
    set: SetFilters,
    pick: fn(&mut ItemFilters, V),
) -> Menu {
    let views = views.to_vec();
    menu.submenu("View", move |sub| view_items(sub.icon(icon), &views, current, set, pick))
}

/// One group of View's choices, the current one checked.
fn view_items<V: Copy + PartialEq + std::fmt::Display + 'static>(
    mut sub: Menu,
    views: &[V],
    current: V,
    set: SetFilters,
    pick: fn(&mut ItemFilters, V),
) -> Menu {
    for &view in views {
        let set = set.clone();
        sub = sub.item(view.to_string(), move |item| {
            item.checked(view == current).action(move || set(Box::new(move |f| pick(f, view))))
        });
    }
    sub
}

/// Repository, shared by the tabs: every one the tab can show, and the
/// one picked even when nothing of it is left, so it can be cleared.
fn repository_submenu(menu: Menu, current: Option<String>, mut repositories: Vec<String>, set: SetFilters) -> Menu {
    if let Some(selected) = current.as_ref() {
        if !repositories.iter().any(|repo| repo.eq_ignore_ascii_case(selected)) {
            repositories.push(selected.clone());
            repositories.sort_by_key(|repo| repo.to_lowercase());
        }
    }
    menu.submenu("Repository", move |mut sub| {
        sub = sub.icon(SidebarIcon::Repository);
        let all = set.clone();
        let all_selected = current.is_none();
        sub = sub.item("All repositories", move |item| {
            item.checked(all_selected).action(move || all(Box::new(|f| f.repository = None)))
        });
        sub = sub.separator();
        for repo in &repositories {
            let set = set.clone();
            let repo = repo.clone();
            let selected = current.as_ref().is_some_and(|current| current.eq_ignore_ascii_case(&repo));
            sub = sub.item(repo.clone(), move |item| {
                item.checked(selected).action(move || {
                    let repo = repo.clone();
                    set(Box::new(move |f| f.repository = Some(repo)))
                })
            });
        }
        sub
    })
}

/// State: Any, Open, or Merged or Closed.
fn state_submenu(menu: Menu, current: StateFilter, set: SetFilters, pick: fn(&mut ItemFilters, StateFilter)) -> Menu {
    menu.submenu("State", move |mut sub| {
        sub = sub.icon(SidebarIcon::Issue);
        for value in StateFilter::ALL {
            let set = set.clone();
            sub = sub.item(value.label(), move |item| {
                item.checked(value == current).action(move || set(Box::new(move |f| pick(f, value))))
            });
        }
        sub
    })
}

/// Sort By: the orders the list can take, then the direction under them.
/// With one order there is nothing to pick but the direction.
fn sort_submenu(
    menu: Menu,
    sorts: &'static [ItemSort],
    current: Order,
    set: SetFilters,
    pick: fn(&mut ItemFilters, Order),
) -> Menu {
    menu.submenu("Sort By", move |mut sub| {
        sub = sub.icon(SidebarIcon::Sort);
        if sorts.len() > 1 {
            for &sort in sorts {
                let set = set.clone();
                sub = sub.item(sort.label(), move |item| {
                    item.checked(sort == current.sort)
                        .action(move || set(Box::new(move |f| pick(f, Order { sort, ..current }))))
                });
            }
            sub = sub.separator();
        }
        for direction in Direction::ALL {
            let set = set.clone();
            sub = sub.item(direction.label(), move |item| {
                item.checked(direction == current.direction)
                    .action(move || set(Box::new(move |f| pick(f, Order { direction, ..current }))))
            });
        }
        sub
    })
}

/// A setting that is on or off, checked when on.
fn toggle(menu: Menu, label: &str, icon: SidebarIcon, on: bool, set: SetFilters, flip: fn(&mut ItemFilters)) -> Menu {
    menu.item(label, move |item| {
        item.icon(icon).checked(on).action(move || set(Box::new(flip)))
    })
}

/// Show PR Status: one setting for every tab's rows.
fn pr_status_toggle(menu: Menu, f: &ItemFilters, set: SetFilters) -> Menu {
    toggle(menu, "Show PR Status", SidebarIcon::PullRequest, f.show_pr_status, set, |f| {
        f.show_pr_status = !f.show_pr_status
    })
}

/// Inbox's menu.
fn inbox_menu(state: AppState, backend: Rc<Backend>) -> Menu {
    let f = state.item_filters.get_untracked();
    let set = setter(state);
    let source = state.inbox_source(f.inbox.view);

    // View is two choices, as Sort By is: the list, then whose threads
    // in it, as Browse's View offers.
    let (view, whose, views) = (f.inbox.view, f.inbox.whose, set.clone());
    let mut menu = Menu::new().submenu("View", move |sub| {
        let sub = view_items(sub.icon(SidebarIcon::Inbox), &InboxView::ALL, view, views.clone(), |f, view| {
            f.inbox.view = view
        });
        view_items(sub.separator(), &BrowseView::ALL, whose, views, |f, whose| f.inbox.whose = whose)
    });
    menu = menu.separator();
    let repositories = source.with_untracked(|threads| crate::inbox::repository_options(threads));
    menu = repository_submenu(menu, f.repository.clone(), repositories, set.clone());

    menu = state_submenu(menu, f.inbox.state, set.clone(), |f, value| f.inbox.state = value);
    menu = menu.separator();
    let order = Order { sort: ItemSort::LastActivity, direction: f.inbox.direction };
    menu = sort_submenu(menu, &[], order, set.clone(), |f, order| f.inbox.direction = order.direction);
    menu = menu.separator();
    menu = toggle(menu, "Unread Only", SidebarIcon::Unread, f.inbox.unread_only, set.clone(), |f| {
        f.inbox.unread_only = !f.inbox.unread_only
    });
    menu = pr_status_toggle(menu, &f, set);
    menu = menu.separator();
    bulk_item(menu, state, backend, f.inbox.view)
}

/// Mark N Done over the rows the inbox is showing, or on Done and Snoozed
/// Move N to Inbox, as each row's own action does there.
fn bulk_item(menu: Menu, state: AppState, backend: Rc<Backend>, view: InboxView) -> Menu {
    let shown = state.visible_inbox();
    let n = shown.len();
    let label = if view.is_put_away() { format!("Move {n} to Inbox") } else { format!("Mark {n} Done") };
    menu.item(label, move |item| {
        item.icon(if view.is_put_away() { SidebarIcon::Undo } else { SidebarIcon::Check })
            .enabled(n > 0)
            .action(move || {
                if view.is_put_away() {
                    for thread in &shown {
                        backend.restore_notification(&thread.id);
                    }
                } else {
                    crate::ui::notifications::done_all(state, &backend, &shown);
                }
            })
    })
}

/// Worktrees' menu. Each view keeps its own Sort By.
fn worktree_menu(state: AppState) -> Menu {
    let f = state.item_filters.get_untracked();
    let set = setter(state);
    let mut menu = view_submenu(
        Menu::new(),
        SidebarIcon::Branch,
        &WorktreeView::ALL,
        f.worktrees.view,
        set.clone(),
        |f, view| f.worktrees.view = view,
    );
    menu = menu.separator();
    let repositories = state
        .repos
        .with_untracked(|repos| state.items.with_untracked(|items| crate::items::repository_options(repos, items)));
    menu = repository_submenu(menu, f.repository.clone(), repositories, set.clone());
    menu = state_submenu(menu, f.worktrees.state, set.clone(), |f, value| f.worktrees.state = value);
    menu = menu.separator();
    menu = sort_submenu(menu, &ItemSort::ALL, f.worktrees.order(), set.clone(), |f, order| {
        *f.worktrees.order_mut() = order
    });
    menu = menu.separator();
    menu = toggle(menu, "Unread Only", SidebarIcon::Unread, f.worktrees.unread_only, set.clone(), |f| {
        f.worktrees.unread_only = !f.worktrees.unread_only
    });
    pr_status_toggle(menu, &f, set)
}

/// Browse's menu. Its Repository offers the declared repositories on
/// GitHub, which is what it can search.
fn browse_menu(state: AppState) -> Menu {
    let f = state.item_filters.get_untracked();
    let set = setter(state);
    let mut menu = view_submenu(
        Menu::new(),
        SidebarIcon::Search,
        &BrowseView::ALL,
        f.browse.view,
        set.clone(),
        |f, view| f.browse.view = view,
    );
    menu = menu.separator();
    let repositories = state.repos.with_untracked(|repos| crate::items::github_repositories(repos));
    menu = repository_submenu(menu, f.repository.clone(), repositories, set.clone());
    menu = state_submenu(menu, f.browse.state, set.clone(), |f, value| f.browse.state = value);
    menu = menu.separator();
    menu = sort_submenu(menu, &ItemSort::BROWSE, f.browse.order, set.clone(), |f, order| f.browse.order = order);
    menu = menu.separator();
    pr_status_toggle(menu, &f, set)
}

/// Inbox is GitHub's ordered thread list. Worktrees' views are over
/// Perch's local items; Browse's are GitHub's searches.
///
/// A poll that would move the rows is held back, and the pill over the
/// list says so until the reader asks for it.
fn item_list(state: AppState, backend: Rc<Backend>, on_open: OpenItem) -> impl IntoView {
    // A memo: every filter change writes the filters, and a container
    // keyed on the signal would build its list again on each.
    let tab = floem::reactive::Memo::new(move |_| state.item_filters.with(|filters| filters.tab));
    Stack::vertical((
        dyn_container(
            move || tab.get(),
            move |tab| match tab {
                SidebarTab::Inbox => crate::ui::notifications::notification_list(state, backend.clone()).into_any(),
                SidebarTab::Worktrees => local_item_list(state, backend.clone(), on_open.clone()).into_any(),
                SidebarTab::Browse => crate::ui::browse::browse(state, backend.clone()).into_any(),
            },
        )
        .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0)),
        crate::ui::widgets::news_pill(
            move || state.held_back_news().unwrap_or_default(),
            move || state.held_back_news().is_some(),
            move || state.show_held_back(),
        ),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .flex_grow(1.0_f32)
            .min_height(0.0)
            .position(floem::style::Position::Relative)
    })
}

/// Worktrees' scrolling list, for either view. Pinning changes order and
/// the row's pin indicator without adding a separate heading.
fn local_item_list(state: AppState, backend: Rc<Backend>, on_open: OpenItem) -> impl IntoView {
    // One list for the empty message and the rows. A memo, so a write
    // that leaves the view's rows as they were diffs nothing. The rows
    // are keyed on the whole item: a row draws most of it.
    let visible = floem::reactive::Memo::new(move |_| state.visible_items());
    Stack::vertical((
        dyn_container(
            move || {
                let view = state.item_filters.with(|f| f.worktrees.view);
                if !visible.with(Vec::is_empty) {
                    None
                } else if state.item_query.with(|q| !q.trim().is_empty()) || has_rows_unnarrowed(state) {
                    Some("No matching items")
                } else {
                    Some(match view {
                        WorktreeView::Worktrees => "No worktrees",
                        WorktreeView::Recent => "Nothing opened yet",
                    })
                }
            },
            |message| {
                if let Some(message) = message {
                    Label::new(message)
                        .style(|s| s.font_size(theme::TEXT_META).color(theme::nav_subtle()).padding(24.0))
                        .into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        ),
        dyn_stack(
            move || visible.get(),
            |item: &Item| item.clone(),
            move |item| item_row(state, backend.clone(), item, on_open.clone()),
        )
        .style(|s| s.flex_col().width_full().gap(1.0)),
    ))
    .style(|s| s.flex_col().width_full().padding_top(18.0).padding_bottom(16.0))
    .scroll()
    .style(|s| s.width_full().flex_grow(1.0_f32).min_height(0.0))
}

/// Whether the view would list anything with the menu's narrowing taken
/// off: whether an empty list is the menu's doing.
fn has_rows_unnarrowed(state: AppState) -> bool {
    let mut filters = state.item_filters.get();
    filters.repository = None;
    filters.worktrees.state = StateFilter::Any;
    filters.worktrees.unread_only = false;
    filters.query.clear();
    state.items.with(|items| !crate::items::local_items(items, &filters, &|_| false).is_empty())
}

/// The strip along the panel's right edge that drags it wider or narrower.
///
/// Sits over the panel's border rather than beside it, so the layout does
/// not gain a gap. The new width is the pointer's distance from the panel's
/// left edge, which is the window's left edge: the lights lane is part of
/// the open drawer, not a rail beside it. No delta bookkeeping, so a fast
/// drag cannot drift.
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
            let width = point.x.clamp(theme::SIDEBAR_MIN_WIDTH, theme::SIDEBAR_MAX_WIDTH);
            state.sidebar_width.set(width);
        })
        .on_event_stop(el::DragEnd, move |_, _: &floem::event::DragEndEvent| {
            state.sidebar_resizing.set(false);
        })
        .on_event_stop(el::DragCancel, move |_, _: &floem::event::DragCancelEvent| {
            state.sidebar_resizing.set(false);
        })
        .debug_name("sidebar resize handle")
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

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};
    use perch_github::{
        Issue, ItemSubject, Reason, Repository, Subject, SubjectKind, SubjectSnapshot, SubjectState,
        Thread,
    };

    fn issue(id: &str, repo: &str, author: &str) -> Item {
        let now = "2026-09-19T12:00:00Z".parse().unwrap();
        let mut item = Item::new(repo, now);
        item.id = id.into();
        item.subject = Some(ItemSubject::Issue(Issue {
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
            number: 1,
            title: format!("Issue {id}"),
            body: String::new(),
            state: "open".into(),
            state_reason: None,
            author_login: author.into(),
            author_avatar_url: String::new(),
            labels: vec![],
            assignees: vec![],
            repo_full_name: repo.into(),
            comments: 0,
            created_at: Some(now),
            updated_at: now,
        }));
        item
    }

    /// A pull request of the reader's own, checked out, with its churn
    /// measured.
    fn pull_request(id: &str, repo: &str, author: &str) -> Item {
        let now = "2026-09-19T12:00:00Z".parse().unwrap();
        let mut item = Item::new(repo, now);
        item.id = id.into();
        item.subject = Some(ItemSubject::PullRequest(perch_github::PullRequest {
            number: 57,
            title: format!("Pull {id}"),
            html_url: String::new(),
            author_login: author.into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "fence".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 10,
            deletions: 2,
            changed_files: 1,
            repo_full_name: repo.into(),
            head_repo_full_name: None,
            updated_at: Some(now),
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        }));
        item.checkout = Some(perch_github::ItemCheckout {
            branch: "fence".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/x".into(),
            owned: true,
            on_disk: true,
            churn: Some((1192, 36)),
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        item
    }

    /// A worktree of the reader's own, checked out, with no pull request.
    fn branch(id: &str, repo: &str) -> Item {
        let mut item = Item::new(repo, "2026-09-19T12:00:00Z".parse().unwrap());
        item.id = id.into();
        item.name = Some(format!("Branch {id}"));
        item.checkout = Some(perch_github::ItemCheckout {
            branch: id.into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: format!("/x/{id}").into(),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
            divergence: None,
            source: None,
        });
        item
    }

    fn notification(id: &str, repo: &str, title: &str, kind: SubjectKind, unread: bool) -> Thread {
        Thread {
            id: id.into(),
            unread,
            local_unread: false,
            local_read: false,
            reason: Reason::Mention,
            updated_at: "2026-09-19T12:00:00Z".parse().unwrap(),
            last_read_at: (!unread).then(|| "2026-09-19T11:00:00Z".parse().unwrap()),
            subject: Subject {
                title: title.into(),
                kind,
                url: Some(format!("https://api.github.com/repos/{repo}/issues/{id}")),
                latest_comment_url: None,
            },
            repository: Repository { full_name: repo.into(), owner_avatar_url: String::new() },
            subject_snapshot: None,
            run_snapshot: None,
            snoozed_until: None,
        }
    }

    /// The list of items, all of it dark once the system is.
    #[test]
    fn the_list_goes_dark_with_the_system() {
        let mut harness = harness().0;
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(left.is_empty(), "light colours left in dark:\n{}", left.join("\n"));
    }

    fn harness() -> (HeadlessHarness, AppState) {
        let root = TestRoot::new();
        let state = AppState::new();
        state.sidebar_width.set(theme::SIDEBAR_MIN_WIDTH);
        state.viewer.set(Some(("me".into(), String::new())));
        // The clock the ages are read against: a day after the rows.
        state.now.set("2026-09-20T12:00:00Z".parse().unwrap());
        state.items.set(vec![
            issue("mine", "org/one", "me"),
            issue("notification", "org/one", "someone"),
            issue("elsewhere", "org/two", "me"),
            pull_request("pr", "org/one", "me"),
            branch("quiet", "org/one"),
        ]);
        // The pull request has been looked at: someone opened it, it
        // changes a few lines, and a review is still wanted.
        let mut looked = notification("11", "org/one", "Unread pull request", SubjectKind::PullRequest, true);
        looked.subject_snapshot = Some(SubjectSnapshot {
            author_login: "someone".into(),
            author_avatar_url: String::new(),
            state: SubjectState::PrMerged,
            churn: Some((3, 1)),
            review_requested: true,
            participants: None,
            at: looked.updated_at,
        });
        state.notifications.set(vec![
            notification("12", "org/one", "Read discussion", SubjectKind::Discussion, false),
            looked,
            notification("10", "org/two", "Other repository", SubjectKind::Issue, true),
        ]);
        state.item_filters.update(|f| {
            f.repository = Some("org/one".into());
            f.tab = SidebarTab::Worktrees;
        });
        let view = sidebar(state, Rc::new(Backend::detached()), Rc::new(|_| {}));
        let mut harness = HeadlessHarness::new_with_size(root, view, theme::SIDEBAR_MIN_WIDTH, 600.0);
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|child| find(child, text))
    }

    fn click_tab(harness: &mut HeadlessHarness, tab: SidebarTab) {
        let id = find(harness.root_id(), &format!("tab:{}", tab.label())).unwrap();
        let rect = id.get_visual_rect_no_clip();
        assert!(rect.width() > 0.0 && rect.height() > 0.0);
        assert!(
            rect.x0 >= 0.0 && rect.x1 <= theme::SIDEBAR_MIN_WIDTH,
            "{} must fit at minimum sidebar width: {rect:?}",
            tab.label()
        );
        let center = rect.center();
        harness.click(center.x, center.y);
        harness.rebuild();
        harness.paint();
    }

    #[test]
    fn tabs_filter_rendered_rows_and_persist_without_clearing_repository() {
        let (mut harness, state) = harness();
        // Worktrees: what is on disk, in the filtered repository.
        assert!(find(harness.root_id(), "Pull pr").is_some());
        assert!(find(harness.root_id(), "item row:quiet").is_some());
        assert!(find(harness.root_id(), "Issue mine").is_none(), "an issue has no worktree");

        click_tab(&mut harness, SidebarTab::Inbox);
        assert_eq!(state.item_filters.get().tab, SidebarTab::Inbox);
        assert!(find(harness.root_id(), "Pull pr").is_none());
        assert!(find(harness.root_id(), "Read discussion").is_some(), "read and unsupported subjects stay listed");
        assert!(find(harness.root_id(), "Unread pull request").is_some());
        assert!(find(harness.root_id(), "Other repository").is_none());
        let (key, value) = state.pending_meta.get().unwrap();
        assert_eq!(key, "item_filters");
        let restored = crate::items::ItemFilters::parse(&value).unwrap();
        assert_eq!(restored.tab, SidebarTab::Inbox);
        assert_eq!(restored.repository.as_deref(), Some("org/one"));

        // Worktrees › Recent: everything opened, worktree or not, in the
        // repository the tabs share.
        click_tab(&mut harness, SidebarTab::Worktrees);
        set_filters(state, |f| f.worktrees.view = WorktreeView::Recent);
        repaint(&mut harness);
        assert!(find(harness.root_id(), "Issue mine").is_some());
        assert!(find(harness.root_id(), "Issue notification").is_some());
        assert!(find(harness.root_id(), "Pull pr").is_some(), "a worktree is in both");
        assert!(find(harness.root_id(), "Issue elsewhere").is_none(), "another repository");
        assert!(find(harness.root_id(), "Read discussion").is_none());
        set_filters(state, |f| f.repository = None);
        repaint(&mut harness);
        assert!(find(harness.root_id(), "Issue elsewhere").is_some());

        // Browse lists GitHub's searches, none of the local items.
        click_tab(&mut harness, SidebarTab::Browse);
        assert!(find(harness.root_id(), "Pull pr").is_none());
        click_tab(&mut harness, SidebarTab::Worktrees);
        assert!(find(harness.root_id(), "Pull pr").is_some());
    }

    fn repaint(harness: &mut HeadlessHarness) {
        harness.rebuild();
        harness.paint();
    }

    /// Inbox's Unread Only leaves the read threads out.
    #[test]
    fn inbox_can_show_only_the_unread() {
        let (mut harness, state) = harness();
        click_tab(&mut harness, SidebarTab::Inbox);
        assert!(find(harness.root_id(), "Read discussion").is_some());
        set_filters(state, |f| f.inbox.unread_only = true);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Read discussion").is_none());
        assert!(find(harness.root_id(), "Unread pull request").is_some());
    }

    /// View › Done lists the threads marked Done instead of the inbox's,
    /// and a row's action there moves it back rather than marking it Done.
    #[test]
    fn inbox_can_show_the_done_and_offers_to_move_them_back() {
        let (mut harness, state) = harness();
        click_tab(&mut harness, SidebarTab::Inbox);
        state.done_notifications.set(vec![notification(
            "20",
            "org/one",
            "Finished thread",
            SubjectKind::Issue,
            false,
        )]);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Finished thread").is_none(), "not in the inbox");

        set_filters(state, |f| f.inbox.view = InboxView::Done);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Read discussion").is_none());
        let row = find(harness.root_id(), "notification row:20").expect("no Done row");
        let rect = row.get_visual_rect_no_clip();
        harness.pointer_move(rect.center().x, rect.center().y);
        harness.rebuild();
        harness.paint();
        let row = find(harness.root_id(), "notification row:20").unwrap();
        assert!(find(row, "move to inbox").is_some(), "no way back on hover");
        assert!(find(row, "mark done").is_none());

        state.done_notifications.set(Vec::new());
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "No notifications marked Done").is_some());
    }

    /// An inbox row offers Snooze on hover, and View › Snoozed lists the
    /// snoozed with when each comes back and a way to move it back.
    #[test]
    fn inbox_rows_snooze_and_show_snoozed_lists_them() {
        let (mut harness, state) = harness();
        click_tab(&mut harness, SidebarTab::Inbox);
        let row = find(harness.root_id(), "notification row:").expect("no inbox row");
        let rect = row.get_visual_rect_no_clip();
        harness.pointer_move(rect.center().x, rect.center().y);
        harness.rebuild();
        harness.paint();
        let row = find(harness.root_id(), "notification row:").unwrap();
        assert!(find(row, "snooze button").is_some(), "no Snooze on hover");
        harness.pointer_move(0.0, 0.0);

        let mut snoozed = notification("30", "org/one", "Put off thread", SubjectKind::Issue, false);
        snoozed.snoozed_until = Some(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(20));
        state.snoozed_notifications.set(vec![snoozed]);
        set_filters(state, |f| f.inbox.view = InboxView::Snoozed);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "Read discussion").is_none());
        let row = find(harness.root_id(), "notification row:30").expect("no snoozed row");
        assert!(find(row, "snoozed until").is_some());
        let rect = row.get_visual_rect_no_clip();
        harness.pointer_move(rect.center().x, rect.center().y);
        harness.rebuild();
        harness.paint();
        let row = find(harness.root_id(), "notification row:30").unwrap();
        assert!(find(row, "move to inbox").is_some());
        assert!(find(row, "snooze button").is_none());

        state.snoozed_notifications.set(Vec::new());
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "No snoozed notifications").is_some());
    }

    /// A worktree whose directory goes leaves Worktrees and stays in
    /// Recent.
    #[test]
    fn a_worktree_that_goes_stays_in_recent() {
        let (mut harness, state) = harness();
        assert!(find(harness.root_id(), "item row:quiet").is_some());
        state.items.update(|items| {
            let quiet = items.iter_mut().find(|i| i.id == "quiet").unwrap();
            quiet.archived_at = Some("2026-09-20T12:00:00Z".parse().unwrap());
        });
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "item row:quiet").is_none());
        set_filters(state, |f| f.worktrees.view = WorktreeView::Recent);
        repaint(&mut harness);
        assert!(find(harness.root_id(), "item row:quiet").is_some());
    }

    /// Every tab's menu has the same shape: View, then the narrowing,
    /// then Sort By, then what is shown and done.
    #[test]
    fn each_tab_s_menu_has_the_same_shape() {
        let (_harness, state) = harness();
        let backend = Rc::new(Backend::detached());
        set_filters(state, |f| f.tab = SidebarTab::Inbox);
        assert_eq!(
            inbox_menu(state, backend.clone()).outline(),
            [
                "View ▸", "—", "Repository ▸", "State ▸", "—", "Sort By ▸", "—", "Unread Only",
                "✓ Show PR Status", "—", "Mark 2 Done",
            ]
        );
        let menu = inbox_menu(state, backend.clone());
        assert_eq!(
            menu.submenu_named("View").unwrap().outline(),
            ["✓ Inbox", "Snoozed", "Done", "—", "Review requested", "Yours", "Assigned", "Mentioned", "✓ All"]
        );
        // The two groups are picked apart.
        menu.submenu_named("View").unwrap().choose("Mentioned");
        assert_eq!(
            inbox_menu(state, backend.clone()).submenu_named("View").unwrap().outline(),
            ["✓ Inbox", "Snoozed", "Done", "—", "Review requested", "Yours", "Assigned", "✓ Mentioned", "All"]
        );
        set_filters(state, |f| f.inbox.whose = BrowseView::All);
        let menu = inbox_menu(state, backend.clone());
        assert_eq!(
            menu.submenu_named("State").unwrap().outline(),
            ["✓ Any", "Open", "Merged or Closed"]
        );
        assert_eq!(menu.submenu_named("Sort By").unwrap().outline(), ["✓ Newest first", "Oldest first"]);
        assert_eq!(
            menu.submenu_named("Repository").unwrap().outline(),
            ["All repositories", "—", "✓ org/one", "org/two"]
        );

        set_filters(state, |f| f.tab = SidebarTab::Worktrees);
        let menu = worktree_menu(state);
        assert_eq!(
            menu.outline(),
            ["View ▸", "—", "Repository ▸", "State ▸", "—", "Sort By ▸", "—", "Unread Only", "✓ Show PR Status"]
        );
        assert_eq!(menu.submenu_named("View").unwrap().outline(), ["✓ Worktrees", "Recent"]);
        assert_eq!(
            menu.submenu_named("Sort By").unwrap().outline(),
            ["✓ Last activity", "Last opened", "Created", "—", "✓ Newest first", "Oldest first"]
        );
        // Recent keeps its own order.
        menu.submenu_named("View").unwrap().choose("Recent");
        assert_eq!(
            worktree_menu(state).submenu_named("Sort By").unwrap().outline(),
            ["Last activity", "✓ Last opened", "Created", "—", "✓ Newest first", "Oldest first"]
        );
        worktree_menu(state).submenu_named("Sort By").unwrap().choose("Created");
        assert_eq!(state.item_filters.get().worktrees.recent.sort, ItemSort::Created);
        assert_eq!(state.item_filters.get().worktrees.worktrees.sort, ItemSort::LastActivity);

        set_filters(state, |f| f.tab = SidebarTab::Browse);
        let menu = browse_menu(state);
        assert_eq!(
            menu.outline(),
            ["View ▸", "—", "Repository ▸", "State ▸", "—", "Sort By ▸", "—", "✓ Show PR Status"]
        );
        assert_eq!(
            menu.submenu_named("View").unwrap().outline(),
            ["Review requested", "Yours", "Assigned", "Mentioned", "✓ All"]
        );
        assert_eq!(
            menu.submenu_named("State").unwrap().outline(),
            ["Any", "✓ Open", "Merged or Closed"]
        );
        assert_eq!(
            menu.submenu_named("Sort By").unwrap().outline(),
            ["✓ Last activity", "Created", "—", "✓ Newest first", "Oldest first"]
        );
    }

    /// State › Merged or Closed narrows the inbox to what a look found
    /// finished, and Mark N Done puts all of those away in one request,
    /// closing the one the main pane may have open.
    #[test]
    fn inbox_narrows_to_merged_and_marks_them_all_done() {
        let (mut harness, state) = harness();
        let (backend, mut commands) = Backend::recording();
        let backend = Rc::new(backend);
        click_tab(&mut harness, SidebarTab::Inbox);
        inbox_menu(state, backend.clone()).submenu_named("State").unwrap().choose("Merged or Closed");
        repaint(&mut harness);
        assert!(find(harness.root_id(), "Unread pull request").is_some(), "merged");
        assert!(find(harness.root_id(), "Read discussion").is_none(), "not looked at, so open");
        assert!(state.item_filters.get().narrowed());
        assert_eq!(state.item_filters.get().inbox.state, StateFilter::Closed);

        while commands.try_recv().is_ok() {}
        inbox_menu(state, backend.clone()).choose("Mark 1 Done");
        let mut done = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let crate::backend::Command::DoneNotifications { thread_ids } = command {
                done.push(thread_ids);
            }
        }
        assert_eq!(done, [vec!["11".to_string()]]);

        // On Done, the same place moves them back instead.
        state.done_notifications.set(vec![notification("20", "org/one", "Finished", SubjectKind::Issue, false)]);
        set_filters(state, |f| {
            f.inbox.view = InboxView::Done;
            f.inbox.state = StateFilter::Any;
        });
        let menu = inbox_menu(state, backend.clone());
        assert_eq!(menu.outline().last().map(String::as_str), Some("Move 1 to Inbox"));

        // With nothing shown there is nothing to mark.
        state.done_notifications.set(Vec::new());
        assert_eq!(inbox_menu(state, backend).outline().last().map(String::as_str), Some("(Move 0 to Inbox)"));
    }

    /// The search box names the list it searches, until there is a
    /// search.
    #[test]
    fn the_search_box_names_the_view() {
        let (mut harness, state) = harness();
        click_tab(&mut harness, SidebarTab::Inbox);
        assert_eq!(state.item_filters.get().view_label(), "Inbox");
        set_filters(state, |f| f.inbox.view = InboxView::Done);
        assert_eq!(state.item_filters.get().view_label(), "Done");
        set_filters(state, |f| f.tab = SidebarTab::Browse);
        assert_eq!(state.item_filters.get().view_label(), "All");
        repaint(&mut harness);
        let shown = find(harness.root_id(), "sidebar search placeholder").unwrap();
        let rect = shown.get_visual_rect_no_clip();
        let input = find(harness.root_id(), "sidebar search").unwrap().get_visual_rect_no_clip();
        assert!(!rect.is_zero_area());
        assert_eq!(rect.x0, input.x0 + 35.0, "where the typed text starts: the border and the padding");
        assert!((rect.center().y - input.center().y).abs() < 1.0, "centred in the box: {rect:?} {input:?}");
        assert!(rect.x1 <= input.x1 - 32.0, "clear of the clear button");
        state.item_query.set("fence".into());
        repaint(&mut harness);
        let shown = find(harness.root_id(), "sidebar search placeholder").unwrap();
        assert!(shown.get_visual_rect_no_clip().is_zero_area(), "gone once there is a search");
    }

    #[test]
    fn a_press_beside_the_lights_is_the_strips_and_the_drawer_control_keeps_its_click() {
        let (mut harness, state) = harness();
        let strip = find(harness.root_id(), "drawer strip").unwrap();
        let y = theme::TITLEBAR_HEIGHT / 2.0;

        // The lane the lights sit in is the strip's own surface: what the
        // window drags from.
        let hit = harness
            .element_id_at(theme::ACTIVITY_RAIL_WIDTH / 2.0, y)
            .expect("nothing under the lane");
        assert_eq!(hit.owning_id(), strip, "the lane took the press");

        let toggle = find(harness.root_id(), "drawer toggle").unwrap().get_visual_rect_no_clip();
        assert_eq!(toggle.x0, theme::ACTIVITY_RAIL_WIDTH, "right after the lights");
        assert!(state.sidebar_open.get());
        harness.click(toggle.center().x, toggle.center().y);
        assert!(!state.sidebar_open.get(), "the control did not shut the drawer");
    }

    #[test]
    fn codex_dock_geometry_holds_at_the_minimum_width() {
        let (mut harness, _) = harness();

        let all = find(harness.root_id(), "tab:Inbox").unwrap().get_visual_rect_no_clip();
        assert_eq!(all.height(), 31.0);
        assert!(
            all.y0 >= 0.0 && all.y1 <= theme::TITLEBAR_HEIGHT,
            "scope leaves title strip: {all:?}"
        );

        let search = find(harness.root_id(), "sidebar search")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!(search.height(), theme::NAV_CONTROL_HEIGHT);
        assert_eq!(search.x0, theme::NAV_GUTTER);

        let filter = find(harness.root_id(), "sidebar filter")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!((filter.width(), filter.height()), (32.0, 32.0));
        assert!(filter.x1 <= theme::SIDEBAR_MIN_WIDTH - theme::NAV_GUTTER);

        let row = find(harness.root_id(), "item row:quiet")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!(row.height(), theme::ROW_HEIGHT);
        assert!(
            row.x0 >= theme::NAV_GUTTER && row.x1 <= theme::SIDEBAR_MIN_WIDTH - theme::NAV_GUTTER,
            "row leaves gutter: {row:?}"
        );

        // A real paint exercises the rounded backgrounds and SVG glyphs in
        // addition to the style and layout passes above.
        harness.paint();
    }

    #[test]
    fn the_clear_button_shows_with_a_query_and_empties_the_box() {
        let (mut harness, state) = harness();
        let clear = find(harness.root_id(), "sidebar search clear").unwrap();
        assert!(clear.get_visual_rect_no_clip().is_zero_area(), "shown with nothing to clear");

        state.item_query.set("wal".into());
        harness.rebuild();
        harness.paint();
        let search = find(harness.root_id(), "sidebar search").unwrap().get_visual_rect_no_clip();
        let rect = clear.get_visual_rect_no_clip();
        assert_eq!((rect.width(), rect.height()), (22.0, 22.0));
        assert!(rect.x1 < search.x1 && rect.x0 > search.center().x, "not at the box's right: {rect:?}");
        assert_eq!(rect.center().y, search.center().y);

        harness.click(rect.center().x, rect.center().y);
        assert_eq!(state.item_query.get(), "");
    }

    /// An empty list says what it is; one emptied by the search says so.
    #[test]
    fn empty_lists_say_why() {
        let (mut harness, state) = harness();
        state.item_query.set("no such item".into());
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "No matching items").is_some());
        assert!(find(harness.root_id(), "Pull pr").is_none());

        state.item_query.set(String::new());
        state.items.set(Vec::new());
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "No worktrees").is_some());
        set_filters(state, |f| f.worktrees.view = WorktreeView::Recent);
        repaint(&mut harness);
        assert!(find(harness.root_id(), "Nothing opened yet").is_some());

        // Emptied by the menu rather than by having nothing.
        state.items.set(vec![issue("elsewhere", "org/two", "me")]);
        repaint(&mut harness);
        assert!(find(harness.root_id(), "No matching items").is_some(), "another repository's");
    }

    /// Over a row, Pin takes the place of the age; Save is gone.
    #[test]
    fn a_hovered_row_offers_pin() {
        let (mut harness, state) = harness();
        let row = find(harness.root_id(), "item row:quiet").unwrap();
        assert!(find(row, "row pin").is_none());
        state.hovered_item.set(Some("quiet".into()));
        harness.rebuild();
        harness.paint();
        let row = find(harness.root_id(), "item row:quiet").unwrap();
        let pin = find(row, "row pin").expect("no pin on hover");
        assert!(pin.get_visual_rect_no_clip().width() > 0.0);
    }

    /// A right-click on a row opens its menu at the pointer, drawn by
    /// Perch, and a row's letter works while it is up, as the native
    /// menu's accelerators did.
    #[test]
    fn a_right_click_opens_the_rows_menu_and_its_letters_work() {
        let (mut harness, state) = harness();
        let row = find(harness.root_id(), "item row:pr").unwrap().get_visual_rect_no_clip();
        let at = row.center();
        harness.secondary_pointer_down(at.x, at.y);
        harness.rebuild();
        harness.paint();
        assert!(crate::ui::menu::is_open());
        let panel = find(harness.root_id(), "menu-panel:0").unwrap().get_visual_rect_no_clip();
        // At the pointer's height, and inside the narrow drawer's window.
        assert_eq!(panel.y0, at.y);
        assert!(panel.x0 >= 0.0 && panel.x1 <= theme::SIDEBAR_MIN_WIDTH, "{panel:?}");
        for name in ["menu-row:Open in", "menu-row:Pin", "menu-row:Rename", "menu-row:Delete"] {
            assert!(find(harness.root_id(), name).is_some(), "no {name}");
        }

        harness.dispatch_event(floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Character("r".into()),
            code: Code::KeyR,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        }));
        harness.rebuild();
        harness.paint();
        assert!(!crate::ui::menu::is_open());
        assert_eq!(state.renaming.get().as_deref(), Some("pr"));
    }

    /// The `...` on a hovered row opens the same menu, under it.
    #[test]
    fn a_rows_more_button_opens_its_menu_under_it() {
        let (mut harness, state) = harness();
        state.hovered_item.set(Some("quiet".into()));
        harness.rebuild();
        harness.paint();
        let row = find(harness.root_id(), "item row:quiet").unwrap();
        let more = find(row, "row more").unwrap().get_visual_rect_no_clip();
        harness.pointer_down(more.center().x, more.center().y);
        harness.rebuild();
        harness.paint();
        let panel = find(harness.root_id(), "menu-panel:0").unwrap().get_visual_rect_no_clip();
        assert!(panel.y0 >= more.y1, "{panel:?} under {more:?}");
        crate::ui::menu::close();
    }

    /// The pointer crossing rows rebuilds only the two rows it crosses,
    /// as it does all the time while the list scrolls under it.
    #[test]
    fn hovering_one_row_leaves_the_others_alone() {
        let (mut harness, state) = harness();
        let age = |harness: &HeadlessHarness| {
            find(find(harness.root_id(), "item row:pr").unwrap(), "row age").unwrap()
        };
        let before = age(&harness);
        state.hovered_item.set(Some("quiet".into()));
        harness.rebuild();
        state.hovered_item.set(None);
        harness.rebuild();
        assert_eq!(age(&harness), before);
    }

    /// A poll with a new row floats a pill over the list and moves
    /// nothing; a click anywhere on the pill, not only on Refresh, puts
    /// the row in.
    #[test]
    fn a_held_back_poll_floats_a_pill_and_refresh_shows_it() {
        let (mut harness, state) = harness();
        let row = find(harness.root_id(), "item row:quiet").unwrap();
        let before = row.get_visual_rect_no_clip();
        let pill = find(harness.root_id(), "news pill").unwrap();
        assert!(pill.get_visual_rect_no_clip().is_zero_area(), "hidden with nothing held");

        let mut polled = state.items.get();
        polled.push(branch("fresh", "org/one"));
        state.apply(crate::backend::Event::ItemsPolled(polled));
        harness.rebuild();
        harness.paint();

        assert!(find(harness.root_id(), "item row:fresh").is_none());
        assert_eq!(row.get_visual_rect_no_clip(), before, "the rows stay put");
        let shown = pill.get_visual_rect_no_clip();
        assert!(shown.width() > 0.0 && shown.height() > 0.0);
        assert!(shown.y1 < before.y1 + 40.0, "the pill is at the top of the list: {shown:?}");

        let refresh = find(harness.root_id(), "news refresh").unwrap().get_visual_rect_no_clip();
        let oval = find(harness.root_id(), "news pill oval").unwrap().get_visual_rect_no_clip();
        let (x, y) = (oval.x0 + 20.0, oval.center().y);
        assert!(x < refresh.x0, "the click lands on the words, left of Refresh");
        harness.click(x, y);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "item row:fresh").is_some());
        assert!(state.pending_items.get().is_none());
    }

    /// Every row of the drawer has Codex's shape: the title with the age
    /// at its right end, then the avatar, the repository, the number,
    /// and the churn under the age. Item rows and notification rows are
    /// the same shape at the same inset, and say the same things.
    #[test]
    fn every_row_is_the_title_and_age_over_the_byline() {
        let (mut harness, _) = harness();
        let row = find(harness.root_id(), "item row:pr").unwrap();
        let rect = row.get_visual_rect_no_clip();
        let title = find(row, "Pull pr").unwrap().get_visual_rect_no_clip();
        let age = find(row, "row age").unwrap().get_visual_rect_no_clip();
        let byline = find(row, "row byline").unwrap().get_visual_rect_no_clip();
        let avatar = find(row, "row avatar").unwrap().get_visual_rect_no_clip();
        let figures = find(row, "row figures").unwrap().get_visual_rect_no_clip();

        assert!(find(row, "1d").is_some(), "the age is a day, short");
        assert!(title.y1 <= byline.y0, "the title is over the byline");
        assert!(title.x1 <= age.x0, "the age is at the title's right end");
        assert_eq!(age.x1, rect.x1 - 13.0, "the age ends at the row's inset");
        assert_eq!(figures.x1, age.x1, "the figures end under the age");
        assert_eq!(avatar.x0, title.x0, "the avatar starts under the title");
        assert_eq!(avatar.width(), crate::ui::nav_row::AVATAR, "the author's, until a look finds who acted");
        assert!(find(row, "org/one").is_some(), "the repository");
        assert!(find(row, "#57").is_some(), "then the number");
        assert!(find(row, "fence").is_none(), "never the branch");
        assert!(find(row, "+1,192").is_some() && find(row, "-36").is_some(), "then the churn");

        // A notification row has the same bones at the same inset, and
        // says the same things of its pull request once it has been
        // looked at.
        click_tab(&mut harness, SidebarTab::Inbox);
        let thread = find(harness.root_id(), "notification row:11").unwrap();
        let thread_rect = thread.get_visual_rect_no_clip();
        assert_eq!((thread_rect.x0, thread_rect.x1), (rect.x0, rect.x1));
        assert_eq!(thread_rect.height(), rect.height());
        let thread_age = find(thread, "row age").unwrap().get_visual_rect_no_clip();
        assert_eq!(thread_age.x1, age.x1);
        assert_eq!(find(thread, "row figures").unwrap().get_visual_rect_no_clip().x1, age.x1);
        assert!(find(thread, "#11").is_some(), "the number");
        assert!(find(thread, "review requested").is_some(), "then the review pill");
        assert!(find(thread, "+3").is_some() && find(thread, "-1").is_some(), "then the churn");
        assert!(find(thread, "mention").is_none(), "the reason is the filter menu's, not the row's");

        // One not looked at yet, and not a pull request, says only where
        // it is: no kind where the number goes, nothing where the churn
        // goes.
        let discussion = find(harness.root_id(), "notification row:12").unwrap();
        assert!(find(discussion, "#12").is_some());
        assert!(find(discussion, SubjectKind::Discussion.wire()).is_none(), "not the kind");
        let figures = find(discussion, "row figures").unwrap().get_visual_rect_no_clip();
        assert_eq!(figures.width(), 0.0, "nothing at the right end");
        assert!(find(discussion, "mention").is_none());
    }

    /// An item row is bold and full black with the dot while the inbox
    /// has an unread notification about its pull request or issue, as the
    /// Notifications row for the same thread is, and steps back to the
    /// read title's lighter weight and grey the moment the thread is
    /// read, without being rebuilt; Mark Unread puts all three back. A row
    /// with nothing in the inbox about it is never bold.
    #[test]
    fn an_item_row_is_bold_and_dotted_while_its_notification_is_unread() {
        use floem::style::{FontWeight, TextColor};

        let (mut harness, state) = harness();
        let mut about_pr =
            notification("57", "ORG/ONE", "About the pull", SubjectKind::PullRequest, true);
        about_pr.subject.url = Some("https://api.github.com/repos/org/one/pulls/57".into());
        state.notifications.update(|threads| threads.push(about_pr.clone()));
        harness.rebuild();
        harness.paint();

        let row = find(harness.root_id(), "item row:pr").unwrap();
        let title = find(row, "Pull pr").unwrap();
        let dot = find(row, "unread dot").unwrap();
        assert_eq!(
            harness.get_computed_style(title).get(FontWeight),
            Some(theme::weight_emphasis()),
            "bold while unread"
        );
        assert_eq!(
            harness.get_computed_style(title).get(TextColor),
            Some(theme::nav_fg()),
            "and the full foreground"
        );
        let dot_rect = dot.get_visual_rect_no_clip();
        assert_eq!(dot_rect.width(), 7.0, "the dot is drawn");
        let age = find(row, "row age").unwrap().get_visual_rect_no_clip();
        assert!(dot_rect.x1 <= age.x0, "before the age");

        let other = find(harness.root_id(), "item row:quiet").unwrap();
        let other_title = find(other, "Branch quiet").unwrap();
        assert_eq!(
            harness.get_computed_style(other_title).get(FontWeight),
            Some(theme::weight_read()),
            "nothing in the inbox about it"
        );
        assert_eq!(
            harness.get_computed_style(other_title).get(TextColor),
            Some(theme::nav_read()),
            "so it is grey too"
        );
        assert_eq!(find(other, "unread dot").unwrap().get_visual_rect_no_clip().width(), 0.0);

        // Viewing the pull request reads the thread; the row follows.
        state.notifications.update(|threads| {
            threads.iter_mut().find(|thread| thread.id == "57").unwrap().unread = false;
        });
        harness.rebuild();
        harness.paint();
        assert_eq!(
            find(harness.root_id(), "item row:pr").unwrap(),
            row,
            "the same row, restyled rather than rebuilt"
        );
        assert_eq!(
            harness.get_computed_style(title).get(FontWeight),
            Some(theme::weight_read()),
            "the read title's weight once read"
        );
        assert_eq!(
            harness.get_computed_style(title).get(TextColor),
            Some(theme::nav_read()),
            "and its grey"
        );
        assert_eq!(dot.get_visual_rect_no_clip().width(), 0.0, "the dot is gone");

        // Mark Unread puts the reminder back, and the row with it.
        state.notifications.update(|threads| {
            let thread = threads.iter_mut().find(|thread| thread.id == "57").unwrap();
            thread.unread = true;
            thread.local_unread = true;
        });
        harness.rebuild();
        harness.paint();
        assert_eq!(
            harness.get_computed_style(title).get(FontWeight),
            Some(theme::weight_emphasis())
        );
        assert_eq!(harness.get_computed_style(title).get(TextColor), Some(theme::nav_fg()));
        assert_eq!(dot.get_visual_rect_no_clip().width(), 7.0);
    }

    /// A notification row is bold and full black with the dot while
    /// GitHub has it unread, and steps back to the read title's lighter
    /// weight and grey once the thread is read.
    #[test]
    fn a_notification_row_unbolds_once_its_thread_is_read() {
        use floem::style::{FontWeight, TextColor};

        let (mut harness, state) = harness();
        click_tab(&mut harness, SidebarTab::Inbox);
        let row = find(harness.root_id(), "notification row:11").unwrap();
        let title = find(row, "Unread pull request").unwrap();
        assert_eq!(harness.get_computed_style(title).get(FontWeight), Some(theme::weight_emphasis()));
        assert_eq!(harness.get_computed_style(title).get(TextColor), Some(theme::nav_fg()));
        assert_eq!(find(row, "unread dot").unwrap().get_visual_rect_no_clip().width(), 7.0);

        state.notifications.update(|threads| {
            threads.iter_mut().find(|thread| thread.id == "11").unwrap().unread = false;
        });
        harness.rebuild();
        harness.paint();
        let row = find(harness.root_id(), "notification row:11").unwrap();
        let title = find(row, "Unread pull request").unwrap();
        assert_eq!(harness.get_computed_style(title).get(FontWeight), Some(theme::weight_read()));
        assert_eq!(harness.get_computed_style(title).get(TextColor), Some(theme::nav_read()));
        assert_eq!(find(row, "unread dot").unwrap().get_visual_rect_no_clip().width(), 0.0);
    }

    #[test]
    fn a_notifications_face_is_whoever_opened_it_and_the_repositorys_until_known() {
        let (mut harness, state) = harness();
        click_tab(&mut harness, SidebarTab::Inbox);

        // No picture has arrived, so each face is its seed's initial.
        let looked = find(harness.root_id(), "notification row:11").unwrap();
        let avatar = find(looked, "row avatar").unwrap();
        assert!(find(avatar, "S").is_some(), "someone's, who opened the pull request");

        let unlooked = find(harness.root_id(), "notification row:12").unwrap();
        let avatar = find(unlooked, "row avatar").unwrap();
        assert!(find(avatar, "O").is_some(), "org's, which nobody opened");

        // Once a look has found who acted there, they are the faces.
        state.notifications.update(|threads| {
            let snapshot = threads.iter_mut().find(|t| t.id == "11").unwrap().subject_snapshot.as_mut().unwrap();
            snapshot.participants = Some(vec![
                perch_github::Participant { login: "zed".into(), avatar_url: String::new() },
                perch_github::Participant { login: "someone".into(), avatar_url: String::new() },
            ]);
        });
        harness.rebuild();
        harness.paint();
        let looked = find(harness.root_id(), "notification row:11").unwrap();
        assert!(find(find(looked, "row face:0").unwrap(), "S").is_some(), "the earlier behind");
        assert!(find(find(looked, "row face:1").unwrap(), "Z").is_some(), "the most recent in front");
    }

    /// Once a look has found who acted last, a row stacks them as
    /// GitHub's inbox does: the most recent at the right and in front,
    /// each face ringed in the row's own colour, which follows the row's
    /// hover.
    #[test]
    fn a_rows_faces_are_who_acted_last_the_most_recent_in_front() {
        use floem::peniko::Brush;
        use floem::style::Background;
        use perch_github::Participant;

        let (mut harness, state) = harness();
        let who = |login: &str| Participant { login: login.into(), avatar_url: String::new() };
        state.apply(crate::backend::Event::Participants(vec![(
            ("org/one".into(), 57),
            vec![who("recent"), who("middle"), who("earliest")],
        )]));
        harness.rebuild();
        harness.paint();

        let row = find(harness.root_id(), "item row:pr").unwrap();
        let faces: Vec<_> = (0..3).map(|i| find(row, &format!("row face:{i}")).unwrap()).collect();
        assert!(find(faces[0], "E").is_some(), "the earliest at the left");
        assert!(find(faces[1], "M").is_some());
        assert!(find(faces[2], "R").is_some(), "the most recent at the right");
        let rects: Vec<_> = faces.iter().map(|f| f.get_visual_rect_no_clip()).collect();
        for pair in rects.windows(2) {
            assert!(pair[0].x0 < pair[1].x0 && pair[1].x0 < pair[0].x1, "the faces overlap: {pair:?}");
        }
        let avatar = find(row, "row avatar").unwrap().get_visual_rect_no_clip();
        let step = crate::ui::nav_row::FACE_STEP;
        assert_eq!(avatar.width(), crate::ui::nav_row::AVATAR + 2.0 * step);

        let ring = |harness: &HeadlessHarness| {
            let row = find(harness.root_id(), "item row:pr").unwrap();
            harness.get_computed_style(find(row, "row face:2").unwrap()).get(Background)
        };
        assert_eq!(ring(&harness), Some(Brush::Solid(theme::nav_canvas())));
        let rect = row.get_visual_rect_no_clip();
        harness.pointer_move(rect.center().x, rect.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(ring(&harness), Some(Brush::Solid(theme::nav_hover())), "the ring follows the row's wash");
    }

    /// The window with no item open, all of it dark once the system is.
    #[test]
    fn goes_dark_with_the_system() {
        let mut harness = window().0;
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(left.is_empty(), "light colours left in dark:\n{}", left.join("\n"));
    }

    /// A downloaded update puts its restart in the login's place, inside
    /// the drawer at its narrowest and beside the footer's buttons, and a
    /// press asks the backend to install it.
    #[test]
    fn a_waiting_update_is_restarted_from_the_footer() {
        let root = TestRoot::new();
        let state = AppState::new();
        state.sidebar_width.set(theme::SIDEBAR_MIN_WIDTH);
        state.viewer.set(Some(("me".into(), String::new())));
        let review = crate::review::model::ReviewState::new();
        let (backend, mut commands) = Backend::recording();
        let view = crate::ui::app_view(state, review, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1200.0, 760.0);
        harness.rebuild();
        harness.paint();
        assert!(find(harness.root_id(), "update-button").is_none());

        state.update.set(UpdateStatus::Ready { version: "0.2.0".into() });
        harness.rebuild();
        harness.paint();
        let button = find(harness.root_id(), "update-button").unwrap().get_visual_rect_no_clip();
        let footer = find(harness.root_id(), "drawer footer").unwrap().get_visual_rect_no_clip();
        let invite = find(harness.root_id(), "invite-button").unwrap().get_visual_rect_no_clip();
        assert!(button.width() > 60.0, "{button:?}");
        assert!(button.y0 >= footer.y0 && button.y1 <= footer.y1, "{button:?} in {footer:?}");
        assert!(button.x1 <= invite.x0, "{button:?} runs into {invite:?}");
        assert!(invite.x1 <= theme::SIDEBAR_MIN_WIDTH, "{invite:?}");

        harness.click(button.center().x, button.center().y);
        assert_eq!(state.update.get(), UpdateStatus::Installing { version: "0.2.0".into() });
        match commands.try_recv() {
            Ok(crate::backend::Command::InstallUpdate { version }) => assert_eq!(version, "0.2.0"),
            other => panic!("expected InstallUpdate, got {other:?}"),
        }
    }

    /// The whole window, so the drawer sits where it does in the app: at
    /// the left edge, with the lights lane inside it.
    fn window() -> (HeadlessHarness, AppState) {
        let root = TestRoot::new();
        let state = AppState::new();
        let review = crate::review::model::ReviewState::new();
        let view = crate::ui::app_view(state, review, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1200.0, 760.0);
        harness.rebuild();
        harness.paint();
        (harness, state)
    }

    fn drawer_right_edge(harness: &HeadlessHarness) -> f64 {
        find(harness.root_id(), "sidebar resize handle")
            .unwrap()
            .get_visual_rect_no_clip()
            .x1
            - 3.0
    }

    #[test]
    fn the_drawer_edge_follows_the_pointer_while_resizing() {
        let (mut harness, state) = window();
        assert_eq!(state.sidebar_width.get(), theme::SIDEBAR_WIDTH);
        assert_eq!(drawer_right_edge(&harness), theme::SIDEBAR_WIDTH);

        // Grab the edge and nudge it one pixel: the drawer must not jump.
        let y = 300.0;
        let grab = theme::SIDEBAR_WIDTH - 1.0;
        harness.pointer_down(grab, y);
        harness.pointer_move(grab + 1.0, y);
        assert!(state.sidebar_resizing.get());
        assert_eq!(state.sidebar_width.get(), grab + 1.0);

        // Then the edge tracks the pointer, in both directions.
        harness.pointer_move(480.0, y);
        assert_eq!(state.sidebar_width.get(), 480.0);
        harness.rebuild();
        harness.paint();
        assert_eq!(drawer_right_edge(&harness), 480.0);
        harness.pointer_move(400.0, y);
        assert_eq!(state.sidebar_width.get(), 400.0);

        // Past either limit the width stops at the limit.
        harness.pointer_move(900.0, y);
        assert_eq!(state.sidebar_width.get(), theme::SIDEBAR_MAX_WIDTH);
        harness.pointer_move(100.0, y);
        assert_eq!(state.sidebar_width.get(), theme::SIDEBAR_MIN_WIDTH);

        harness.pointer_up(100.0, y);
        assert!(!state.sidebar_resizing.get());
        assert_eq!(state.sidebar_width.get(), theme::SIDEBAR_MIN_WIDTH);
    }
}
