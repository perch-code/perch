//! Code's tabs, shaped like an editor's: the open files'.
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
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::cards::tip_with;
use crate::review::model::{ItemTab, Layout, Origin, ReviewState};
use crate::review::timeline::count_of;
use crate::state::AppState;
use crate::theme;
use crate::ui::drawer;
use crate::ui::widgets::icon_in;

/// The open files' tabs, then, at the right end, what is about the open
/// file while it shows: the way back from a retained snapshot, the notice
/// about what the files are when it is not the pull request's latest, the
/// change navigation and the file's tiles; then the drawer's. The way back
/// and the notice are here because a file has no summary to hold them; on
/// Conversation, the summary has them both.
pub fn tab_bar(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // A memo, so the file's notes are built again when a file comes to
    // show or goes, and not on every write to the tab.
    let showing_file = floem::reactive::Memo::new(move |_| on_file(state));
    Stack::horizontal((
        dyn_container(move || state.tabs.with(|t| t.is_empty()), {
            let backend = backend.clone();
            move |empty| {
                if empty {
                    strip_rule().style(|s| s.size_full()).into_any()
                } else {
                    open_tabs(state, backend.clone()).into_any()
                }
            }
        })
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
        // Built only while a file shows, since there is one of each.
        dyn_container(move || showing_file.get(), {
            let backend = backend.clone();
            move |showing: bool| {
                if !showing {
                    return Empty::new().into_any();
                }
                Stack::horizontal((
                    crate::review::summary::snapshot_banner(app, state, backend.clone())
                        .style(|s| s.margin_left(8.0)),
                    crate::review::summary::notice(app, state, backend.clone())
                        .style(|s| s.margin_horiz(8.0)),
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
                .border_color(theme::border())
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
            .background(theme::canvas_subtle())
    })
}

/// Whether the pane shows Code, rather than Conversation. Tracked.
fn on_file(state: ReviewState) -> bool {
    state.tab.get() == ItemTab::Code && state.has_files()
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
    // floem keeps a bar's width at the right whether or not a bar shows,
    // which cut the rule short of the controls.
    .style(|s| s.height_full().width_full().scrollbar_width(0.0))
}

fn tab_view(
    state: ReviewState,
    backend: Rc<Backend>,
    path: String,
    origin: Origin,
) -> impl IntoView {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();

    // Active while the pane shows this file, not merely while it is the
    // file the pane would show.
    let (active_box, active_text) = (path.clone(), path.clone());
    let is_active_box = move || {
        state.tab.get() == ItemTab::Code
            && state
                .active
                .with(|a| a.as_deref() == Some(active_box.as_str()))
    };
    let is_active_text = move || {
        state.tab.get() == ItemTab::Code
            && state
                .active
                .with(|a| a.as_deref() == Some(active_text.as_str()))
    };

    let dot_path = path.clone();
    let select_path = path.clone();
    let close_path = path.clone();
    let debug_name = format!("file tab:{path}");

    let tab = Stack::horizontal((
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
    // Its content is asked for when it is not here, as for a tab an item
    // opened on again whose load has failed.
    .on_event_stop(el::Click, move |_, _| {
        show_file(state, &backend, &select_path)
    })
    .style(move |s| tab_style(s, is_active_box()));

    // Where the file is on disk, since the tab has room for its name only.
    tip_with(tab, move || on_disk(state, &path)).style(|s| s.height_full().flex_shrink(0.0_f32))
}

/// A tab's file as a full path: a worktree file's is under the checkout,
/// and an outside file's is absolute already.
fn on_disk(state: ReviewState, path: &str) -> String {
    let root = state.item.with(|i| {
        i.as_ref()
            .and_then(|i| i.checkout.as_ref().map(|c| c.path.clone()))
    });
    match root {
        Some(root) => root.join(path).display().to_string(),
        None => path.to_string(),
    }
}

/// Show a file's tab, from Conversation too, in one rebuild of the pane
/// rather than one for each write. Its content is asked for when it is
/// not here. Nothing for the file already showing: opening it again
/// would rebuild the editor, which flickered and lost the scroll a beat.
/// A tab names the file the reader wants, as a link does, so Code stays
/// over a composer drawn in the file it leaves.
fn show_file(state: ReviewState, backend: &Backend, path: &str) {
    let showing = state.tab.get_untracked() == ItemTab::Code
        && state.active.with_untracked(|a| a.as_deref() == Some(path));
    if showing && (state.file(path).is_some() || state.is_loading(path)) {
        return;
    }
    let load = floem::reactive::Effect::batch(|| {
        state.one_move(|| {
            state.show_tab(ItemTab::Code);
            state.open_for_jump(|| state.open_tab(path))
        })
    });
    if load {
        crate::review::load_file(backend, path);
    }
}

/// The files' tabs, when there are files to have them.
fn strip(state: ReviewState) -> Vec<String> {
    if state.has_files() {
        state
            .tabs
            .with_untracked(|t| t.iter().map(|t| t.path.clone()).collect())
    } else {
        Vec::new()
    }
}

/// Show the file's tab after the one showing, or before it, wrapping at
/// the ends, as Safari's Cmd-} and Cmd-{ do across its tabs. From
/// Conversation, the first or the last. False when there is no tab to go
/// to.
pub fn step_tab(state: ReviewState, backend: &Backend, forward: bool) -> bool {
    let files = strip(state);
    let count = files.len();
    if count == 0 {
        return false;
    }
    let current = match state.tab.get_untracked() {
        ItemTab::Code => state.active.with_untracked(|active| {
            active
                .as_deref()
                .and_then(|a| files.iter().position(|p| p == a))
        }),
        ItemTab::Conversation | ItemTab::Changescape => None,
    };
    let next = match current {
        Some(i) if forward => (i + 1) % count,
        Some(i) => (i + count - 1) % count,
        None if forward => 0,
        None => count - 1,
    };
    show_file(state, backend, &files[next]);
    true
}

/// Room in the strip with nothing in it, ruled along its bottom as the
/// inactive tabs are, so the rule under the strip runs unbroken to the
/// active tab.
fn strip_rule() -> Empty {
    Empty::new().style(|s| {
        s.flex_grow(1.0_f32)
            .height_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}

/// A file's tab. The active one has the surface of
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
        .border_color(theme::border_muted())
        .hover(|s| s.cursor(CursorStyle::Pointer));

    if active {
        base.background(theme::code_bg())
            .border_top(2.0)
            .border_color(theme::accent())
    } else {
        base.background(Color::TRANSPARENT)
            .border_bottom(1.0)
            .border_color(theme::border())
            .hover(|s| s.background(theme::canvas_hover()))
    }
}

/// A tab's name: dark while it shows, muted otherwise.
fn tab_label(s: Style, active: bool) -> Style {
    s.font_size(theme::TEXT_META)
        .color(if active {
            theme::fg()
        } else {
            theme::fg_muted()
        })
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
            .color(theme::fg_muted())
            .height(theme::ORIGIN_CHIP_HEIGHT)
            .padding_horiz(5.0)
            .items_center()
            .border_radius(4.0)
            .background(theme::badge_bg())
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
        crate::ui::widgets::sidebar_glyph(crate::icons::SidebarIcon::Close, theme::fg_subtle)
            .style(|s| s.size(11.0, 11.0)),
    )
    .on_event_stop(el::Click, move |_, _| on_close())
    .style(|s| {
        s.size(17.0, 17.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .hover(|s| s.background(theme::badge_bg()).cursor(CursorStyle::Pointer))
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
                Label::derived(move || count_of(n, "change", "changes")).style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::fg_muted())
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
            .border_color(theme::border())
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
    let Some(file) = state.active_file() else {
        return;
    };
    let from = state.cursor_line.get();
    let target = if forward {
        file.buffer.next_change(from)
    } else {
        file.buffer.previous_change(from)
    };
    if let Some(display_line) = target {
        state.cursor_line.set(display_line);
        // The editor scrolls by head line, so translate back through the
        // row table; a ghost line has none, so use the line after.
        let head = file
            .buffer
            .rows
            .iter()
            .skip(display_line)
            .find_map(|r| r.head_line())
            .unwrap_or(display_line);
        state.jump_to_line.set(Some(head));
    }
}

fn nav_button(state: ReviewState, forward: bool) -> impl IntoView {
    use crate::ui::widgets::sidebar_glyph;

    let glyph = if forward {
        SidebarIcon::ChevronDown
    } else {
        SidebarIcon::ChevronUp
    };

    Container::new(sidebar_glyph(glyph, theme::fg_muted))
        .on_event_stop(el::Click, move |_, _| jump_to_change(state, forward))
        .style(|s| {
            s.size(20.0, 20.0)
                .items_center()
                .justify_center()
                .border_radius(4.0)
                .hover(|s| s.background(theme::badge_bg()).cursor(CursorStyle::Pointer))
        })
}

/// The tiles at the strip's right end: while a file shows, Source, when
/// the file is a picture, then Show whole, then Stacked and Split, with a
/// rule after each group; then the tree in the drawer on the right. A
/// chosen tile is filled;
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
        Stack::horizontal((tree_tile(app, backend),)).style(|s| s.items_center().gap(2.0)),
    ))
    .style(|s| {
        s.flex_shrink(0.0_f32)
            .height_full()
            .items_center()
            .padding_horiz(8.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}

/// The rule between two groups of tiles.
fn tile_rule() -> impl IntoView {
    Empty::new().style(|s| {
        s.width(1.0)
            .height(theme::TILE_RULE_HEIGHT)
            .margin_horiz(theme::TILE_RULE_GAP)
            .flex_shrink(0.0_f32)
            .background(theme::border_muted())
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
        tile(SidebarIcon::Source, shows_source, move || {
            state.toggle_source()
        })
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

fn layout_tile(
    app: AppState,
    state: ReviewState,
    layout: Layout,
    glyph: SidebarIcon,
) -> impl IntoView {
    let active = move || state.layout.get() == layout;
    tile(glyph, active, move || {
        // A write of the same value would still rebuild the editor.
        if state.layout.get_untracked() == layout {
            return;
        }
        state.layout.set(layout);
        app.pending_meta
            .set(Some(("review.layout".into(), layout.name().into())));
    })
    .debug_name(format!("{} tile", layout.name()))
}

/// The tree in the drawer on the right: filled while it shows, and a
/// click shuts it; otherwise a click opens it. Nothing to give the
/// keyboard back to afterwards: a tile is not a control it can rest on.
fn tree_tile(app: AppState, backend: std::rc::Rc<Backend>) -> impl IntoView {
    let active = move || app.tree_shown();
    tip_with(
        tile(SidebarIcon::Folder, active, move || {
            drawer::toggle(app, &backend, None)
        })
        .debug_name("files tile"),
        move || {
            if active() {
                "Hide the files".to_string()
            } else {
                "Show the files".to_string()
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
    tip_with(
        tile(SidebarIcon::Fold, folded, move || state.toggle_whole()),
        move || {
            if folded() {
                "Folded to the changes. Click to show the file whole.".to_string()
            } else {
                "Shown whole. Click to fold to the changes.".to_string()
            }
        },
    )
}

/// One `LAYOUT_TILE` square: filled while `active`, a glyph otherwise.
fn tile(
    glyph: SidebarIcon,
    active: impl Fn() -> bool + Copy + 'static,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    let color = move || {
        if active() {
            theme::fg()
        } else {
            theme::fg_muted()
        }
    };

    Container::new(icon_in(
        move || crate::icons::sidebar_icon(glyph, color()),
        color,
    ))
    .on_event_stop(el::Click, move |_, _| on_click())
    .style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .size(theme::LAYOUT_TILE, theme::LAYOUT_TILE)
            .items_center()
            .justify_center()
            .border_radius(4.0);
        if active() {
            base.background(theme::badge_bg())
        } else {
            base.hover(|s| s.background(theme::canvas_hover()))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::review::header::tests::{issue_harness, named, pull_request_harness, rect, within};
    use crate::review::model::ItemTab;

    /// The tree's tile, after the layout tiles: a click opens the tree in
    /// the drawer, under the tabs, and the body moves over to make room; a
    /// click on the filled tile shuts it. The tiles stay put over it. On
    /// Conversation the tree is hidden, and comes back with Code.
    #[test]
    fn the_tree_tile_opens_and_shuts_the_tree_beside_the_pane() {
        let (mut harness, app, state) = pull_request_harness();
        // As a real open leaves it: `opening` stays set while the item is.
        state.opening.set(true);
        // The sidebar shut, so the window has the room to dock, and a file
        // open on Code, where the tiles are, on a visit after the item's
        // first, so Code does not open the tree itself.
        app.sidebar_open.set(false);
        state.open_tab("README.md");
        state.code_seen.set(true);
        state.show_tab(ItemTab::Code);
        state.tree.set(Arc::new(vec![perch_git::TreeEntry::new(
            "README.md",
            false,
        )]));
        harness.rebuild();
        harness.paint();
        assert!(!app.tree_shown());
        let tabs = rect(&harness, "tab strip");
        assert_eq!(tabs.x1, 1400.0);
        let split = rect(&harness, "split tile");
        let files = rect(&harness, "files tile");
        assert!(split.x1 <= files.x0, "after the layout tiles");
        assert!(within(files, tabs));
        assert!(
            named(harness.root_id(), "agents tile").is_none(),
            "the agents have no tile"
        );

        harness.click(files.center().x, files.center().y);
        harness.rebuild();
        harness.paint();
        assert!(app.tree_shown());
        let drawer = rect(&harness, "right-drawer");
        assert_eq!(drawer.x1, 1400.0);
        assert_eq!(drawer.width(), theme::DRAWER_WIDTH);
        assert_eq!(drawer.y0, tabs.y1, "the drawer starts under the tabs");
        assert_eq!(
            rect(&harness, "tab strip"),
            tabs,
            "the tabs run over the drawer"
        );
        assert_eq!(rect(&harness, "files tile"), files, "the tile stays put");
        assert_eq!(
            rect(&harness, "pane-body").x1,
            drawer.x0,
            "the body stops where the drawer starts"
        );
        let close = rect(&harness, "files-close");
        assert!(within(close, drawer));
        assert!(
            close.y1 <= drawer.y0 + theme::DRAWER_HEADER_HEIGHT,
            "the close control is in the drawer's bar: {close:?}"
        );
        let row = rect(&harness, "tree-row:README.md");
        assert!(within(row, drawer));
        assert!(row.y0 >= drawer.y0 + theme::DRAWER_HEADER_HEIGHT + theme::SCOPE_BUTTON_HEIGHT);

        // Conversation hides it, keeping the wish; Code has it again.
        state.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        assert!(app.tree_open.get() && !app.tree_shown());
        assert_eq!(rect(&harness, "right-drawer").width(), 0.0);
        state.show_tab(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        assert_eq!(rect(&harness, "right-drawer"), drawer);

        // The filled tile shuts it, and the pane has the window again.
        harness.click(files.center().x, files.center().y);
        harness.rebuild();
        harness.paint();
        assert!(!app.tree_open.get());
        assert!(named(harness.root_id(), "files-panel").is_none());
        assert_eq!(rect(&harness, "pane-body").x1, 1400.0);
        assert_eq!(state.tab.get(), ItemTab::Code);
    }

    /// A tree asked for stays shut on an issue, which has no Code, and
    /// while the next item is on its way; the wish is kept for Code.
    #[test]
    fn a_tree_stays_shut_on_an_issue_and_keeps_its_wish() {
        let (mut harness, app, state) = issue_harness();
        app.sidebar_open.set(false);
        app.tree_open.set(true);
        harness.rebuild();
        harness.paint();
        assert!(!app.tree_available());
        assert!(!app.tree_shown());
        assert_eq!(
            rect(&harness, "pane-body").x1,
            1400.0,
            "the body has the window"
        );

        state.begin("Reading the issue");
        harness.rebuild();
        harness.paint();
        assert!(!app.tree_available(), "an item on its way has no tree");
        assert!(app.tree_open.get() && !app.tree_shown());
        assert_eq!(
            rect(&harness, "pane-body").x1,
            1400.0,
            "the body has the window"
        );
    }

    /// Code's strip is the files' tabs, the first at its left edge, then
    /// the file's tiles and the tree's. Conversation has no tab in it.
    /// The rule after the tabs runs on from the last one.
    #[test]
    fn codes_strip_is_the_files_tabs_then_the_tiles() {
        let (mut harness, _, state) = pull_request_harness();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        let strip = rect(&harness, "tab strip");
        let file = rect(&harness, "file tab:src/x.rs");
        assert_eq!(file.x0, strip.x0, "the file's tab is first");
        assert_eq!(
            (file.y0, file.y1),
            (strip.y0, strip.y1),
            "a file's tab fills the strip"
        );
        assert!(
            named(harness.root_id(), "chat").is_none(),
            "no Conversation in the strip"
        );
        assert!(rect(&harness, "file tiles").width() > 0.0);
        assert!(rect(&harness, "file tiles").x1 <= rect(&harness, "files tile").x0);
        let rule = rect(&harness, "tabs rule");
        assert_eq!(rule.x0, file.x1);
        assert!(rule.width() > 0.0);
        assert_eq!(rule.y1, strip.y1);
    }

    /// A click on the tab already showing does nothing: no rebuild of the
    /// editor, and the caret stays.
    #[test]
    fn a_click_on_the_showing_tab_does_nothing() {
        let (mut harness, _, state) = pull_request_harness();
        state.open_tab("src/x.rs");
        state.show_tab(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        state.cursor_line.set(7);
        let epoch = state.editor_epoch.get_untracked();

        let tab = rect(&harness, "file tab:src/x.rs");
        harness.click(tab.center().x, tab.center().y);
        harness.rebuild();
        assert_eq!(state.editor_epoch.get_untracked(), epoch);
        assert_eq!(state.cursor_line.get_untracked(), 7);
    }

    /// A click on another file's tab, with a composer drawn in the code
    /// of the one showing, shows that file on Code. The composer waits in
    /// its file, drawn there again when it shows.
    #[test]
    fn a_click_on_a_tab_beside_a_composer_in_the_code_stays_on_code() {
        let (mut harness, _, state) = pull_request_harness();
        state.open_tab("slatedb/src/config.rs");
        state.open_tab("bindings/uniffi/src/config.rs");
        for path in ["slatedb/src/config.rs", "bindings/uniffi/src/config.rs"] {
            state.finish_load(
                path,
                Arc::new(crate::backend::build_loaded_file(
                    path.into(),
                    "a\nb\nc\n".into(),
                    "a\nb\n".into(),
                    Some(perch_git::ChangeStatus::Modified),
                    false,
                    perch_git::Scope::Branch,
                    String::new(),
                )),
            );
        }
        state.show_tab(ItemTab::Code);
        let anchor = perch_github::Anchor {
            path: "bindings/uniffi/src/config.rs".into(),
            start_line: 3,
            line: 3,
            side: perch_github::Side::Right,
            commit_sha: "h".into(),
        };
        let choice = (
            perch_agent::Backend::ClaudeCode,
            "sonnet".to_string(),
            perch_agent::Effort::Medium,
        );
        state.open_composer(None, Some(anchor), "why?".into(), choice);
        harness.rebuild();
        harness.paint();
        assert!(state.composer_in_code());

        let tab = rect(&harness, "file tab:slatedb/src/config.rs");
        harness.click(tab.center().x, tab.center().y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.tab.get(), ItemTab::Code);
        assert_eq!(state.active.get().as_deref(), Some("slatedb/src/config.rs"));
        assert_eq!(state.composer_draft.get(), "why?");

        let tab = rect(&harness, "file tab:bindings/uniffi/src/config.rs");
        harness.click(tab.center().x, tab.center().y);
        harness.rebuild();
        assert!(state.composer_in_code());
    }

    #[test]
    fn show_whole_is_the_open_tabs_alone_and_goes_with_the_tab() {
        let state = ReviewState::new();
        let mut item =
            perch_github::Item::new("slatedb/slatedb", "2026-09-18T09:59:00Z".parse().unwrap());
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
