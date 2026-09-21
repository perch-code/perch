//! Open file tabs, shaped like an editor's.
//!
//! The active tab takes the editor's own background and an accent stripe
//! along its top, so it reads as continuous with the code below it. Only the
//! inactive tabs carry the rule that separates the strip from the editor;
//! drawing that rule under the active tab too would cut it off from the file
//! it is showing.

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, CustomStylable};
use floem::views::scroll::ScrollCustomStyle;
use floem::views::{
    Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack,
};

use crate::icons::SidebarIcon;
use crate::review::cards::tip_with;
use crate::review::model::{Layout, Origin, ReviewState};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::icon_in;

/// The open files as tabs, then, at the right end, the notice about what
/// the files are when it is not the pull request's latest, the change
/// navigation and the layout toggle.
pub fn tab_bar(app: AppState, state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        dyn_container(
            move || state.tabs.with(|t| t.is_empty()),
            move |empty| {
                if empty {
                    Empty::new().style(|s| s.size_full()).into_any()
                } else {
                    open_tabs(state).into_any()
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
        crate::review::summary::notice(state).style(|s| s.margin_horiz(8.0)),
        change_nav(state),
        layout_toggle(app, state),
    ))
    .debug_name("file tabs")
    .style(|s| {
        s.height(theme::TAB_HEIGHT)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .background(theme::CANVAS_SUBTLE)
    })
}

fn open_tabs(state: ReviewState) -> impl IntoView {
    dyn_stack(
        move || state.tabs.get(),
        |tab| tab.path.clone(),
        move |tab| tab_view(state, tab.path, tab.origin),
    )
    .style(|s| s.items_center().height_full())
    .scroll()
    // No visible bar on a strip this short: it would sit over the tab names,
    // and the wheel still scrolls. A trackpad's vertical scroll moves the
    // strip sideways, which is the only direction it can go.
    .custom_style(|s: ScrollCustomStyle| s.hide_bars(true).vertical_scroll_as_horizontal(true))
    .style(|s| s.height_full().width_full())
}

fn tab_view(state: ReviewState, path: String, origin: Origin) -> impl IntoView {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();

    let (active_box, active_text) = (path.clone(), path.clone());
    let is_active_box = move || state.active.with(|a| a.as_deref() == Some(active_box.as_str()));
    let is_active_text = move || state.active.with(|a| a.as_deref() == Some(active_text.as_str()));

    let dot_path = path.clone();
    let count_path = path.clone();
    let select_path = path.clone();
    let close_path = path.clone();

    Stack::horizontal((
        language_dot(state, dot_path),
        Label::derived(move || name.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if is_active_text() { theme::FG } else { theme::FG_MUTED })
                .text_ellipsis()
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .margin_right(6.0)
        }),
        // Where a file outside the worktree is from, beside its name.
        match origin.label() {
            Some(label) => origin_chip(label.to_string())
                .style(|s| s.margin_right(6.0))
                .into_any(),
            None => Empty::new().into_any(),
        },
        change_count(state, count_path),
        close_button(state, close_path),
    ))
    .on_event_stop(el::Click, move |_, _| {
        state.open_tab(&select_path);
    })
    .style(move |s| {
        let active = is_active_box();
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
            // Same surface as the editor, with the stripe on top. Nothing
            // along the bottom, so the tab and the file join up.
            base.background(theme::CODE_BG)
                .border_top(2.0)
                .border_color(theme::ACCENT)
        } else {
            base.background(Color::TRANSPARENT)
                .border_bottom(1.0)
                .border_color(theme::BORDER)
                .hover(|s| s.background(theme::CANVAS_HOVER))
        }
    })
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

/// How much of this file the pull request touched.
fn change_count(state: ReviewState, path: String) -> impl IntoView {
    let count = move || state.file(&path).map(|f| f.change_count()).unwrap_or(0);

    dyn_container(count, move |n| {
        if n == 0 {
            Empty::new().into_any()
        } else {
            Label::derived(move || n.to_string())
                .style(|s| {
                    s.font_size(theme::TEXT_BADGE)
                        .color(theme::STATUS_MODIFIED)
                        .selectable(false)
                        .margin_right(5.0)
                })
                .into_any()
        }
    })
    .style(|s| s.flex_shrink(0.0_f32))
}

fn close_button(state: ReviewState, path: String) -> impl IntoView {
    Container::new(
        crate::ui::widgets::sidebar_glyph(crate::icons::SidebarIcon::Close, theme::FG_SUBTLE)
            .style(|s| s.size(11.0, 11.0)),
    )
    .on_event_stop(el::Click, move |_, _| state.close_tab(&path))
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
/// this until it is shown as markup; its tab still carries the count.
fn change_nav(state: ReviewState) -> impl IntoView {
    let count = move || {
        if state.drawn_picture().is_some() {
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

/// The tiles at the strip's right end: Source, while the open file is a
/// picture, then Show whole, then Stacked and Split, with a rule between
/// each group. A chosen tile is filled; the others are only a glyph until
/// the pointer is over one.
///
/// The rules are there because the groups are kinds of preference. The
/// layout outlives the item: it is a way of reading, not a fact about a
/// file, so it goes to `meta` under one key and comes back at the next
/// launch. Show whole is the open tab's: a reader unfolds the file they
/// are reading and not the others, so it lasts as long as the tab and
/// every file opens folded. Source is the file's, for the app's life: a
/// reader wants the markup of this SVG, not of SVGs.
fn layout_toggle(app: AppState, state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        source_group(state),
        fold_tile(state),
        tile_rule(),
        Stack::horizontal((
            layout_tile(app, state, Layout::Stacked, SidebarIcon::Stacked),
            layout_tile(app, state, Layout::Split, SidebarIcon::Split),
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

        // The item closing forgets them all.
        state.toggle_whole();
        assert!(!state.folded("src/b.rs"));
        state.close();
        assert!(state.whole.get().is_empty());
    }
}
