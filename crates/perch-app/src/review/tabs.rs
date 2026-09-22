//! The item's tabs, shaped like an editor's: Conversation's, then the open
//! files'.
//!
//! The active tab takes the surface of what it shows, the editor's or the
//! page's, and an accent stripe along its top, so it reads as continuous
//! with what is below it. Everything in the strip but the active tab
//! carries the rule that separates the strip from the pane; drawing that
//! rule under the active tab too would cut it off from what it is showing.

use std::rc::Rc;

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, CustomStylable, Style};
use floem::views::scroll::ScrollCustomStyle;
use floem::views::{
    Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards::tip_with;
use crate::review::model::{ItemTab, Layout, Origin, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::drawer::{self, DrawerView};
use crate::ui::widgets::icon_in;

/// Conversation's tab and the open files', then, at the right end, what
/// is about the open file while it shows: the way back from a retained
/// snapshot, the notice about what the files are when it is not the pull
/// request's latest, the change navigation and the file's tiles; then the
/// tiles every tab has. The way back and the notice are here because a
/// file has no summary to hold them; on Conversation, the summary has
/// them both.
pub fn tab_bar(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // A memo, so the file's notes are built again when a file comes to
    // show or goes, and not on every write to the tab.
    let showing_file = floem::reactive::Memo::new(move |_| on_file(state));
    Stack::horizontal((
        conversation_tab(state),
        dyn_container(
            move || state.tabs.with(|t| t.is_empty()),
            {
                let backend = backend.clone();
                move |empty| {
                    if empty {
                        strip_rule().style(|s| s.size_full()).into_any()
                    } else {
                        open_tabs(state, backend.clone()).into_any()
                    }
                }
            },
        )
        // A zero basis, so the tabs take the room the notice and the
        // controls leave rather than their full width, which would have
        // squeezed the notice to its glyph; the strip scrolls sideways
        // for the rest.
        .style(|s| {
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height_full()
        }),
        // Built only while a file shows: on Conversation the summary has
        // them, and there is one of each.
        dyn_container(move || showing_file.get(), {
            let backend = backend.clone();
            move |showing: bool| {
                if !showing {
                    return Empty::new().into_any();
                }
                Stack::horizontal((
                    crate::review::summary::snapshot_banner(app, state, backend.clone())
                        .style(|s| s.margin_left(8.0)),
                    crate::review::summary::notice(state).style(|s| s.margin_horiz(8.0)),
                ))
                .style(|s| s.items_center().min_width(0.0))
                .into_any()
            }
        })
        .style(|s| {
            s.items_center()
                .min_width(0.0)
                .height_full()
                .border_bottom(1.0)
                .border_color(theme::BORDER)
        }),
        change_nav(state),
        tiles(app, state, backend),
    ))
    .debug_name("tab strip")
    .style(|s| {
        s.height(theme::TAB_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .background(theme::CANVAS_SUBTLE)
    })
}

/// Whether the pane shows a file's tab, rather than Conversation or
/// nothing. Tracked.
fn on_file(state: ReviewState) -> bool {
    state.tab.get() == ItemTab::Files && state.has_files()
}

/// Conversation's tab, before the files': a tab as theirs are, with the
/// chat glyph where a file has its language's dot. Closed as theirs are;
/// the chat tile, or Cmd-Shift-1, opens it again. Outside the files'
/// scroll, so it stays in sight however many files are open.
fn conversation_tab(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || state.conversation_open.get(),
        move |open| {
            if !open {
                return Empty::new().into_any();
            }
            let active = move || state.tab.get() == ItemTab::Conversation;
            Stack::horizontal((
                crate::ui::widgets::sidebar_glyph(SidebarIcon::Chat, theme::FG_MUTED).style(|s| {
                    s.size(14.0, 14.0)
                        .flex_shrink(0.0_f32)
                        .margin_right(6.0)
                }),
                Label::derived(|| "Conversation".to_string())
                    .style(move |s| tab_label(s, active())),
                close_button(move || state.close_conversation())
                    .debug_name("close chat"),
            ))
            .debug_name("chat tab")
            .on_event_stop(el::Click, move |_, _| state.show_tab(ItemTab::Conversation))
            .style(move |s| tab_style(s, active()))
            .into_any()
        },
    )
    .style(|s| s.height_full().flex_shrink(0.0_f32))
}

fn open_tabs(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // The strip's height given outright: a scroll leaves its content's
    // height open, so a full one came out at the tabs' own and they
    // stopped short of the pane. The room after the last tab carries the
    // rule on to the controls.
    Stack::horizontal((
        dyn_stack(
            move || state.tabs.get(),
            |tab| tab.path.clone(),
            move |tab| tab_view(state, backend.clone(), tab.path, tab.origin),
        )
        .style(|s| s.items_center().height_full()),
        strip_rule().debug_name("tabs rule"),
    ))
    .style(|s| s.height(theme::TAB_HEIGHT).min_width_full())
    .scroll()
    // No visible bar on a strip this short: it would sit over the tab names,
    // and the wheel still scrolls. A trackpad's vertical scroll moves the
    // strip sideways, which is the only direction it can go.
    .custom_style(|s: ScrollCustomStyle| s.hide_bars(true).vertical_scroll_as_horizontal(true))
    .style(|s| s.height_full().width_full())
}

fn tab_view(state: ReviewState, backend: Rc<Backend>, path: String, origin: Origin) -> impl IntoView {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();

    // Active while the pane shows this file, not merely while it is the
    // file the pane would show.
    let (active_box, active_text) = (path.clone(), path.clone());
    let is_active_box = move || {
        state.tab.get() == ItemTab::Files
            && state.active.with(|a| a.as_deref() == Some(active_box.as_str()))
    };
    let is_active_text = move || {
        state.tab.get() == ItemTab::Files
            && state.active.with(|a| a.as_deref() == Some(active_text.as_str()))
    };

    let dot_path = path.clone();
    let select_path = path.clone();
    let close_path = path.clone();
    let debug_name = format!("file tab:{path}");

    Stack::horizontal((
        language_dot(state, dot_path),
        Label::derived(move || name.clone()).style(move |s| tab_label(s, is_active_text())),
        // Where a file outside the worktree is from, beside its name.
        match origin.label() {
            Some(label) => origin_chip(label.to_string())
                .style(|s| s.margin_right(6.0))
                .into_any(),
            None => Empty::new().into_any(),
        },
        close_button(move || state.close_tab(&close_path)).debug_name("close file"),
    ))
    .debug_name(debug_name)
    // From Conversation, a file's tab goes back to the file, in one
    // rebuild of the pane rather than one for each write. Its content is
    // asked for when it is not here, as for a tab an item opened on again
    // whose load has failed.
    .on_event_stop(el::Click, move |_, _| {
        let load = floem::reactive::Effect::batch(|| {
            state.show_tab(ItemTab::Files);
            state.open_tab(&select_path)
        });
        if load {
            crate::review::load_file(&backend, &select_path);
        }
    })
    .style(move |s| tab_style(s, is_active_box()))
}

/// Room in the strip with nothing in it, ruled along its bottom as the
/// inactive tabs are, so the rule under the strip runs unbroken to the
/// active tab.
fn strip_rule() -> Empty {
    Empty::new().style(|s| {
        s.flex_grow(1.0_f32)
            .height_full()
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// A tab, a file's or Conversation's. The active one has the surface of
/// what it shows, the editor's or the page's, which are one white, with
/// the stripe on top and nothing along its bottom, so the tab and what it
/// shows join up.
fn tab_style(s: Style, active: bool) -> Style {
    let base = s
        .height_full()
        .items_center()
        .padding_left(10.0)
        .padding_right(6.0)
        .max_width(theme::TAB_MAX_WIDTH)
        .flex_shrink(0.0_f32)
        .selectable(false)
        .border_right(1.0)
        .border_color(theme::BORDER_MUTED)
        .hover(|s| s.cursor(CursorStyle::Pointer));

    if active {
        base.background(theme::CODE_BG)
            .border_top(2.0)
            .border_color(theme::ACCENT)
    } else {
        base.background(Color::TRANSPARENT)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
            .hover(|s| s.background(theme::CANVAS_HOVER))
    }
}

/// A tab's name: dark while it shows, muted otherwise.
fn tab_label(s: Style, active: bool) -> Style {
    s.font_size(theme::TEXT_META)
        .color(if active { theme::FG } else { theme::FG_MUTED })
        .text_ellipsis()
        .flex_grow(1.0_f32)
        .min_width(0.0)
        .margin_right(6.0)
}

/// The chip naming where a file outside the worktree is from: `tokio
/// 1.40.0`, `std`, `git: floem`. In the muted colour, so it reads as a
/// note on the name and not as a badge.
pub fn origin_chip(label: String) -> impl IntoView {
    Label::derived(move || label.clone()).style(|s| {
        s.font_size(theme::TEXT_SECTION)
            .color(theme::FG_MUTED)
            .height(theme::ORIGIN_CHIP_HEIGHT)
            .padding_horiz(5.0)
            .items_center()
            .border_radius(4.0)
            .background(theme::BADGE_BG)
            .flex_shrink(0.0_f32)
            .selectable(false)
    })
}

/// A dot coloured by language, standing in for a file-type icon.
fn language_dot(state: ReviewState, path: String) -> impl IntoView {
    let color = move || theme::language_color(state.file(&path).and_then(|f| f.language));

    Empty::new().style(move |s| {
        s.size(7.0, 7.0)
            .border_radius(4.0)
            .flex_shrink(0.0_f32)
            .margin_right(7.0)
            .background(color())
    })
}

fn close_button(on_close: impl Fn() + 'static) -> impl IntoView {
    Container::new(
        crate::ui::widgets::sidebar_glyph(crate::icons::SidebarIcon::Close, theme::FG_SUBTLE)
            .style(|s| s.size(11.0, 11.0)),
    )
    .on_event_stop(el::Click, move |_, _| on_close())
    .style(|s| {
        s.size(17.0, 17.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
    })
}

/// Jump between the changed regions of the open file.
///
/// The count is the point as much as the buttons: it says at a glance how
/// much of this file the pull request actually touched. A file drawn as a
/// picture has no lines on the screen to jump between, so it has none of
/// this until it is shown as markup, and neither does Conversation.
fn change_nav(state: ReviewState) -> impl IntoView {
    let count = move || {
        if !on_file(state) || state.drawn_picture().is_some() {
            return 0;
        }
        state.active_file().map(|f| f.change_count()).unwrap_or(0)
    };

    dyn_container(count, move |n| {
        if n == 0 {
            Empty::new().into_any()
        } else {
            Stack::horizontal((
                nav_button(state, false),
                Label::derived(move || {
                    if n == 1 {
                        "1 change".to_string()
                    } else {
                        format!("{n} changes")
                    }
                })
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::FG_MUTED)
                        .selectable(false)
                        .margin_horiz(6.0)
                }),
                nav_button(state, true),
            ))
            .style(|s| s.items_center().padding_horiz(10.0).flex_shrink(0.0_f32))
            .into_any()
        }
    })
    .style(|s| {
        s.flex_shrink(0.0_f32)
            .height_full()
            .items_center()
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// Scroll to the next or previous change after the cursor's line, as the
/// strip's chevrons and the `]` and `[` keys both do. A change is never
/// folded, so the jump reveals nothing; it goes the one way every jump
/// goes so that there is one. Nothing for a file drawn as a picture: a
/// jump left waiting would land in the next editor built, on some other
/// file.
pub fn jump_to_change(state: ReviewState, forward: bool) {
    if state.drawn_picture().is_some() {
        return;
    }
    let Some(file) = state.active_file() else { return };
    let from = state.cursor_line.get();
    let target = if forward { file.buffer.next_change(from) } else { file.buffer.previous_change(from) };
    if let Some(display_line) = target {
        state.cursor_line.set(display_line);
        // The editor scrolls by head line, so translate back through the
        // row table; a ghost line has none, so use the line after.
        let head = file.buffer.rows.iter().skip(display_line).find_map(|r| r.head_line()).unwrap_or(display_line);
        state.jump_to_line.set(Some(head));
    }
}

fn nav_button(state: ReviewState, forward: bool) -> impl IntoView {
    use crate::ui::widgets::sidebar_glyph;

    let glyph = if forward { SidebarIcon::ChevronDown } else { SidebarIcon::ChevronUp };

    Container::new(sidebar_glyph(glyph, theme::FG_MUTED))
        .on_event_stop(el::Click, move |_, _| jump_to_change(state, forward))
        .style(|s| {
            s.size(20.0, 20.0)
                .items_center()
                .justify_center()
                .border_radius(4.0)
                .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
        })
}

/// The tiles at the strip's right end: while a file shows, Source, when
/// the file is a picture, then Show whole, then Stacked and Split, with a
/// rule after each group; then, on every tab, Conversation and the drawer
/// on the right, on the files or on the agents. A chosen tile is filled;
/// the others are only a glyph until the pointer is over one. The last
/// group is at the strip's end, so its tiles keep their place when the
/// file's come and go.
///
/// The rules are there because the groups are kinds of preference. The
/// layout outlives the item: it is a way of reading, not a fact about a
/// file, so it goes to `meta` under one key and comes back at the next
/// launch. Show whole is the open tab's: a reader unfolds the file they
/// are reading and not the others, so it lasts as long as the tab and
/// every file opens folded. Source is the file's, for the app's life: a
/// reader wants the markup of this SVG, not of SVGs. The drawer is the
/// window's, and comes back at the next launch as the layout does.
/// Conversation's tile is no preference but a place to go, as the
/// drawer's are, and sits with them.
fn tiles(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::horizontal((
        Stack::horizontal((
            source_group(state),
            fold_tile(state),
            tile_rule(),
            Stack::horizontal((
                layout_tile(app, state, Layout::Stacked, SidebarIcon::Stacked),
                layout_tile(app, state, Layout::Split, SidebarIcon::Split),
            ))
            .style(|s| s.items_center().gap(2.0)),
            tile_rule(),
        ))
        .debug_name("file tiles")
        .style(move |s| s.items_center().apply_if(!on_file(state), |s| s.hide())),
        Stack::horizontal((
            conversation_tile(state),
            // An item with no files has no files for the drawer to hold.
            drawer_tile(app, backend.clone(), DrawerView::Files)
                .style(move |s| s.apply_if(!state.has_files(), |s| s.hide())),
            drawer_tile(app, backend, DrawerView::Agents),
        ))
        .style(|s| s.items_center().gap(2.0)),
    ))
    .style(|s| {
        s.flex_shrink(0.0_f32)
            .height_full()
            .items_center()
            .padding_horiz(8.0)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// The rule between two groups of tiles.
fn tile_rule() -> impl IntoView {
    Empty::new().style(|s| {
        s.width(1.0)
            .height(theme::TILE_RULE_HEIGHT)
            .margin_horiz(theme::TILE_RULE_GAP)
            .flex_shrink(0.0_f32)
            .background(theme::BORDER_MUTED)
    })
}

/// The Source tile and its rule, while the open file has a picture, and
/// nothing while it does not: the tile means nothing for a file that is
/// only ever its text.
fn source_group(state: ReviewState) -> impl IntoView {
    // A memo, so the group is rebuilt when the answer changes and not on
    // every file that loads.
    let has_picture = floem::reactive::Memo::new(move |_| {
        state.active_file().is_some_and(|f| f.picture.is_some())
    });
    dyn_container(
        move || has_picture.get(),
        move |picture| {
            if picture {
                Stack::horizontal((source_tile(state), tile_rule()))
                    .debug_name("source group")
                    .style(|s| s.items_center())
                    .into_any()
            } else {
                Empty::new().into_any()
            }
        },
    )
    .style(|s| s.items_center().flex_shrink(0.0_f32))
}

/// The markup of a picture, filled while it is what shows; the picture
/// is the default and is only the glyph.
fn source_tile(state: ReviewState) -> impl IntoView {
    let shows_source = move || {
        state
            .active
            .with(|a| a.as_deref().is_some_and(|path| state.shows_source(path)))
    };
    tip_with(
        tile(SidebarIcon::Source, shows_source, move || state.toggle_source())
            .debug_name("source tile"),
        move || {
            if shows_source() {
                "Shown as markup. Click to draw the picture.".to_string()
            } else {
                "Drawn as a picture. Click to show the markup.".to_string()
            }
        },
    )
}

fn layout_tile(app: AppState, state: ReviewState, layout: Layout, glyph: SidebarIcon) -> impl IntoView {
    let active = move || state.layout.get() == layout;
    tile(glyph, active, move || {
        // A write of the same value would still rebuild the editor.
        if state.layout.get_untracked() == layout {
            return;
        }
        state.layout.set(layout);
        app.pending_meta.set(Some(("review.layout".into(), layout.name().into())));
    })
    .debug_name(format!("{} tile", layout.name()))
}

/// Conversation, filled while it shows. A click shows it, and opens its
/// tab again when it was closed.
fn conversation_tile(state: ReviewState) -> impl IntoView {
    let active = move || state.tab.get() == ItemTab::Conversation;
    tip_with(
        tile(SidebarIcon::Chat, active, move || {
            state.show_tab(ItemTab::Conversation)
        })
        .debug_name("chat tile"),
        move || {
            if active() {
                "Showing the conversation".to_string()
            } else {
                "Show the conversation".to_string()
            }
        },
    )
}

/// The drawer on the right, on one of the things it holds: filled while
/// it shows that. A click on the filled tile shuts the drawer; on the
/// other, the drawer opens on it, or swaps to it. Nothing to give the
/// keyboard back to afterwards: a tile is not a control it can rest on.
fn drawer_tile(app: AppState, backend: std::rc::Rc<Backend>, view: DrawerView) -> impl IntoView {
    let active = move || app.drawer_shown() == Some(view);
    tip_with(
        tile(view.glyph(), active, move || {
            drawer::toggle(app, &backend, view, None)
        })
        .debug_name(format!("{} tile", view.what())),
        move || {
            if active() {
                format!("Hide the {}", view.what())
            } else {
                format!("Show the {}", view.what())
            }
        },
    )
}

/// The open file folded to its changes, which is the default and is drawn
/// filled, or shown whole. With no file open it is drawn as the default
/// and does nothing.
fn fold_tile(state: ReviewState) -> impl IntoView {
    let folded = move || {
        state
            .active
            .with(|a| a.as_deref().is_none_or(|path| state.folded(path)))
    };
    tip_with(tile(SidebarIcon::Fold, folded, move || state.toggle_whole()), move || {
        if folded() {
            "Folded to the changes. Click to show the file whole.".to_string()
        } else {
            "Shown whole. Click to fold to the changes.".to_string()
        }
    })
}

/// One `LAYOUT_TILE` square: filled while `active`, a glyph otherwise.
fn tile(glyph: SidebarIcon, active: impl Fn() -> bool + Copy + 'static, on_click: impl Fn() + 'static) -> impl IntoView {
    let color = move || if active() { theme::FG } else { theme::FG_MUTED };

    Container::new(icon_in(move || crate::icons::sidebar_icon(glyph, color()), color))
        .on_event_stop(el::Click, move |_, _| on_click())
        .style(move |s| {
            let base = crate::ui::widgets::clickable(s)
                .size(theme::LAYOUT_TILE, theme::LAYOUT_TILE)
                .items_center()
                .justify_center()
                .border_radius(4.0);
            if active() {
                base.background(theme::BADGE_BG)
            } else {
                base.hover(|s| s.background(theme::CANVAS_HOVER))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::review::header::tests::{
        issue_harness, named, pull_request_harness, rect, within,
    };
    use crate::review::model::ItemTab;

    /// The drawer's two tiles, after the layout tiles: a click opens the
    /// drawer on the files or on the agents, and the pane moves over to
    /// make room; a click on the filled one shuts it, and the other swaps
    /// what it holds. The pane keeps the tab it was on through all of it.
    #[test]
    fn the_drawer_tiles_open_swap_and_shut_the_drawer_beside_the_pane() {
        let (mut harness, app, state) = pull_request_harness();
        // The sidebar shut, so the window has the room to dock, and
        // Files, where the tiles are, with a worktree of one file.
        app.sidebar_open.set(false);
        state.show_tab(ItemTab::Files);
        state.tree.set(Arc::new(vec![perch_git::TreeEntry::new("README.md", false)]));
        harness.rebuild();
        harness.paint();
        assert_eq!(app.drawer_shown(), None);
        let tabs = rect(&harness, "tab strip");
        assert_eq!(tabs.x1, 1400.0);
        let split = rect(&harness, "split tile");
        let files = rect(&harness, "files tile");
        let agents = rect(&harness, "agents tile");
        assert!(split.x1 <= files.x0 && files.x1 <= agents.x0, "after the layout tiles");
        assert!(within(files, tabs) && within(agents, tabs));

        harness.click(files.center().x, files.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(app.drawer_shown(), Some(DrawerView::Files));
        let drawer = rect(&harness, "right-drawer");
        assert_eq!(drawer.x1, 1400.0);
        assert_eq!(drawer.width(), theme::DRAWER_WIDTH);
        assert_eq!(drawer.y0, 0.0, "the drawer starts at the window's top");
        assert_eq!(
            rect(&harness, "tab strip").x1,
            drawer.x0,
            "the pane stops where the drawer starts"
        );
        assert!(named(harness.root_id(), "files-panel").is_some(), "the drawer holds the files");
        let close = rect(&harness, "files-close");
        assert!(within(close, drawer));
        assert!(
            close.y1 <= theme::TITLEBAR_HEIGHT,
            "the drawer's bar is on the title bar's line: {close:?}"
        );
        // The rail's tree is there, from the drawer's bar down.
        let row = rect(&harness, "tree-row:README.md");
        assert!(within(row, drawer));
        assert!(row.y0 >= theme::TITLEBAR_HEIGHT + theme::SCOPE_BUTTON_HEIGHT);

        // Agents: the drawer swaps what it holds, and stays put. The
        // tiles moved over with the strip.
        let agents = rect(&harness, "agents tile");
        harness.click(agents.center().x, agents.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(app.drawer_shown(), Some(DrawerView::Agents));
        assert!(named(harness.root_id(), "files-panel").is_none());
        assert!(named(harness.root_id(), "agents-panel").is_some());
        assert_eq!(rect(&harness, "right-drawer"), drawer);

        // The filled tile shuts it, and the pane has the window again.
        harness.click(agents.center().x, agents.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(app.drawer_shown(), None);
        assert!(named(harness.root_id(), "agents-panel").is_none());
        assert_eq!(rect(&harness, "tab strip").x1, 1400.0);
        assert_eq!(state.tab.get(), ItemTab::Files);
    }

    /// A drawer asked to show the files stays shut on an issue, which has
    /// none, and comes back on the next item with some; the wish is kept.
    #[test]
    fn a_files_drawer_stays_shut_on_an_issue_and_keeps_its_wish() {
        let (mut harness, app, state) = issue_harness();
        app.sidebar_open.set(false);
        app.drawer.set(Some(DrawerView::Files));
        harness.rebuild();
        harness.paint();
        assert!(!app.files_available());
        assert_eq!(app.drawer_shown(), None);
        assert_eq!(rect(&harness, "item bar").x1, 1400.0, "the pane has the window");

        // Agents can still be shown, and the wish for Files is replaced.
        drawer::show(app, &crate::backend::Backend::detached(), DrawerView::Agents, None);
        harness.rebuild();
        harness.paint();
        assert_eq!(app.drawer_shown(), Some(DrawerView::Agents));
        assert_eq!(rect(&harness, "item bar").x1, 1400.0 - theme::DRAWER_WIDTH);

        // While another item is on its way the drawer's column stays, so
        // opening one pull request after another does not move the pane
        // twice; once it is an issue, the column goes.
        app.drawer.set(Some(DrawerView::Files));
        state.begin("Reading the issue");
        harness.rebuild();
        harness.paint();
        assert!(app.files_available(), "an item on its way may have files");
        assert_eq!(app.drawer_shown(), Some(DrawerView::Files));
    }

    /// Conversation is a tab before the files', and the chat tile at the
    /// strip's end shows it. Closed, its tab goes and the file shows; the
    /// tile opens it again, in its place before the files. The file's
    /// tiles are there only while a file shows, and the tiles after them
    /// keep their place when they come and go.
    #[test]
    fn conversation_is_a_tab_that_the_chat_tile_shows_and_opens_again() {
        let (mut harness, _, state) = pull_request_harness();
        state.open_tab("src/x.rs");
        harness.rebuild();
        harness.paint();
        let strip = rect(&harness, "tab strip");
        let conversation = rect(&harness, "chat tab");
        let file = rect(&harness, "file tab:src/x.rs");
        assert_eq!(conversation.x0, strip.x0, "Conversation's tab is first");
        assert!(within(conversation, strip) && within(file, strip));
        assert!(conversation.x1 <= file.x0);
        assert_eq!((file.y0, file.y1), (strip.y0, strip.y1), "a file's tab fills the strip, as Conversation's does");
        assert_eq!((conversation.y0, conversation.y1), (strip.y0, strip.y1));
        let tile = rect(&harness, "chat tile");
        assert!(tile.x1 <= rect(&harness, "files tile").x0);
        assert!(rect(&harness, "files tile").x1 <= rect(&harness, "agents tile").x0);
        assert_eq!(
            rect(&harness, "file tiles").width(),
            0.0,
            "a file's tiles on Conversation"
        );
        assert!(named(harness.root_id(), "conversation-title").is_some());

        // The file's tab shows the file, and its tiles come; the chat
        // tile stays where it was.
        harness.click(file.center().x, file.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.tab.get(), ItemTab::Files);
        assert!(rect(&harness, "file tiles").width() > 0.0);
        assert_eq!(rect(&harness, "chat tile"), tile);
        assert!(named(harness.root_id(), "conversation-title").is_none());
        // The room after the tab is ruled, from the tab to the controls
        // and down to the pane.
        let rule = rect(&harness, "tabs rule");
        assert_eq!(rule.x0, file.x1);
        assert_eq!(rule.y1, strip.y1);
        assert!(rule.width() > 0.0);

        // Back to Conversation, and closed from its tab: the file shows.
        harness.click(conversation.center().x, conversation.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        let close = rect(&harness, "close chat");
        assert!(within(close, conversation));
        harness.click(close.center().x, close.center().y);
        harness.rebuild();
        harness.paint();
        assert!(!state.conversation_open.get());
        assert_eq!(state.tab.get(), ItemTab::Files);
        assert!(named(harness.root_id(), "chat tab").is_none());
        assert_eq!(
            rect(&harness, "file tab:src/x.rs").x0,
            strip.x0,
            "the file's tab moved into the room"
        );

        // The chat tile opens it again, before the file's, and shows it.
        harness.click(tile.center().x, tile.center().y);
        harness.rebuild();
        harness.paint();
        assert!(state.conversation_open.get());
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert_eq!(rect(&harness, "chat tab").x0, strip.x0);
        assert_eq!(rect(&harness, "file tab:src/x.rs"), file);
        assert!(named(harness.root_id(), "conversation-title").is_some());
    }

    /// An issue has Conversation's tab as every item does, and of the
    /// tiles only Conversation's and the agents': it has no files.
    #[test]
    fn an_issue_has_conversations_tab_and_no_tiles_for_files() {
        let (harness, ..) = issue_harness();
        let strip = rect(&harness, "tab strip");
        assert!(within(rect(&harness, "chat tab"), strip));
        assert!(within(rect(&harness, "chat tile"), strip));
        assert!(within(rect(&harness, "agents tile"), strip));
        assert_eq!(rect(&harness, "files tile").width(), 0.0);
        assert_eq!(rect(&harness, "file tiles").width(), 0.0);
    }

    #[test]
    fn show_whole_is_the_open_tabs_alone_and_goes_with_the_tab() {
        let state = ReviewState::new();
        let mut item = perch_github::Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
        item.id = "wt".into();
        state.item.set(Some(Arc::new(item)));

        // Nothing open: nothing to unfold.
        state.toggle_whole();
        assert!(state.whole.get().is_empty());

        state.open_tab("src/a.rs");
        state.open_tab("src/b.rs");
        assert!(state.folded("src/a.rs"), "folded by default");
        assert!(state.folded("src/b.rs"));

        // Show whole on b, the open one: a stays folded.
        state.toggle_whole();
        assert!(!state.folded("src/b.rs"));
        assert!(state.folded("src/a.rs"));

        // Back to a: it is as it was, and b stays whole behind it. A file
        // opened now opens folded.
        state.open_tab("src/a.rs");
        assert!(state.folded("src/a.rs"));
        assert!(!state.folded("src/b.rs"));
        state.open_tab("src/c.rs");
        assert!(state.folded("src/c.rs"));

        // Whole on a click, folded again on the next.
        state.toggle_whole();
        assert!(!state.folded("src/c.rs"));
        state.toggle_whole();
        assert!(state.folded("src/c.rs"));

        // Closing b's tab forgets its Show whole: opened again, it opens
        // folded.
        state.close_tab("src/b.rs");
        assert!(state.folded("src/b.rs"));
        state.open_tab("src/b.rs");
        assert!(state.folded("src/b.rs"));

        // The item closing puts them all away, with its pane.
        state.toggle_whole();
        assert!(!state.folded("src/b.rs"));
        state.close();
        assert!(state.whole.get().is_empty());
    }
}
