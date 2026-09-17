//! The file rail: changed files, or the whole worktree.


use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::text::FontWeight;
use floem::views::{Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack};
use perch_git::{ChangeStatus, ChangedFile, TreeEntry};

use crate::review::model::{RailMode, ReviewState};
use crate::review::tree;
use crate::review::OpenFile;
use crate::icons::SidebarIcon;
use crate::ui::widgets::sidebar_glyph;
use crate::theme;

pub fn rail(state: ReviewState, on_open: OpenFile) -> impl IntoView {
    Stack::vertical((
        mode_toggle(state),
        dyn_container(
            move || state.rail_mode.get(),
            move |mode| match mode {
                RailMode::Changed => changed_list(state, on_open.clone()).into_any(),
                RailMode::AllFiles => tree_list(state, on_open.clone()).into_any(),
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full()),
    ))
    .style(|s| {
        s.width(theme::RAIL_WIDTH)
            .height_full()
            .flex_col()
            .flex_shrink(0.0_f32)
            .background(theme::RAIL_BG)
            .border_right(1.0)
            .border_color(theme::BORDER)
    })
}

fn mode_toggle(state: ReviewState) -> impl IntoView {
    let changed_count = move || state.changed_files.with(|f| f.len());

    Stack::horizontal((
        segment(state, RailMode::Changed, "Changed", Some(changed_count)),
        segment(state, RailMode::AllFiles, "All files", None::<fn() -> usize>),
    ))
    .style(|s| {
        s.height(40.0)
            .width_full()
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(8.0)
            .gap(4.0)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

fn segment(
    state: ReviewState,
    mode: RailMode,
    text: &'static str,
    count: Option<impl Fn() -> usize + 'static + Copy>,
) -> impl IntoView {
    let active = move || state.rail_mode.get() == mode;

    Stack::horizontal((
        Label::derived(move || text).style(move |s| {
            s.font_size(theme::TEXT_META)
                .font_weight(if active() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                .color(if active() { theme::FG } else { theme::FG_MUTED })
        }),
        dyn_container(
            move || count.map(|c| c()).unwrap_or(0),
            move |n| {
                if n == 0 {
                    Empty::new().into_any()
                } else {
                    Label::derived(move || n.to_string())
                        .style(|s| {
                            s.font_size(theme::TEXT_BADGE)
                                .color(theme::BADGE_FG)
                                .background(theme::BADGE_BG)
                                .border_radius(8.0)
                                .padding_horiz(5.0)
                                .margin_left(6.0)
                        })
                        .into_any()
                }
            },
        ),
    ))
    .on_event_stop(el::Click, move |_, _| state.rail_mode.set(mode))
    .style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .height(28.0)
            .items_center()
            .padding_horiz(11.0)
            .border_radius(theme::RADIUS)
            ;
        if active() {
            base.background(theme::CANVAS)
                .border(1.0)
                .border_color(theme::BORDER)
        } else {
            base.border(1.0).border_color(Color::TRANSPARENT)
        }
    })
}

fn changed_list(state: ReviewState, on_open: OpenFile) -> impl IntoView {
    dyn_stack(
        move || state.changed_files.get().as_ref().clone(),
        |f| f.path.clone(),
        move |f| changed_row(state, f, on_open.clone()),
    )
    .style(|s| s.flex_col().width_full().padding_vert(4.0))
    .scroll()
    .style(|s| s.size_full().min_height(0.0))
}

fn changed_row(state: ReviewState, file: ChangedFile, on_open: OpenFile) -> impl IntoView {
    let click_path = file.path.clone();
    // One closure per use: each captures a String, so none of them is Copy.
    let (name_path, row_path) = (file.path.clone(), file.path.clone());
    let active_name = move || state.active.with(|a| a.as_deref() == Some(name_path.as_str()));
    let active_row = move || state.active.with(|a| a.as_deref() == Some(row_path.as_str()));

    let directory = file.directory().to_string();
    let name = file.file_name().to_string();
    let status = file.status;
    let additions = file.additions;
    let deletions = file.deletions;
    let binary = file.binary;

    Stack::horizontal((
        status_letter(status),
        // The directory is muted so the eye lands on the file name.
        Stack::horizontal((
            Label::derived(move || directory.clone()).style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .min_width(0.0)
            }),
            Label::derived(move || name.clone()).style(move |s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG)
                    .font_weight(if active_name() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                    .flex_shrink(0.0_f32)
            }),
        ))
        .style(|s| s.flex_grow(1.0_f32).min_width(0.0).items_center()),
        counts(additions, deletions, binary),
    ))
    .on_event_stop(el::Click, move |_, _| on_open(&click_path))
    .style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .height(30.0)
            .width_full()
            .items_center()
            .padding_horiz(10.0)
            .hover(|s| s.background(theme::CANVAS_HOVER));
        if active_row() {
            base.background(theme::ACCENT_SUBTLE)
        } else {
            base
        }
    })
}

fn status_letter(status: ChangeStatus) -> impl IntoView {
    let color = match status {
        ChangeStatus::Added => theme::STATUS_ADDED,
        ChangeStatus::Modified | ChangeStatus::TypeChanged => theme::STATUS_MODIFIED,
        ChangeStatus::Deleted => theme::STATUS_DELETED,
        ChangeStatus::Renamed | ChangeStatus::Copied => theme::STATUS_RENAMED,
    };
    Label::derived(move || status.letter()).style(move |s| {
        s.font_size(theme::TEXT_BADGE)
            .font_weight(FontWeight::SEMI_BOLD)
            .color(color)
            .width(14.0)
            .flex_shrink(0.0_f32)
            .text_align(floem::text::Alignment::Center)
            .margin_right(6.0)
    })
}

fn counts(additions: u32, deletions: u32, binary: bool) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || if binary { "bin".to_string() } else { format!("+{additions}") })
            .style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(if binary { theme::FG_SUBTLE } else { theme::STATUS_ADDED })
            }),
        Label::derived(move || if binary { String::new() } else { format!("-{deletions}") }).style(
            |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::STATUS_DELETED)
                    .margin_left(5.0)
            },
        ),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32).margin_left(8.0))
}

fn tree_list(state: ReviewState, on_open: OpenFile) -> impl IntoView {
    dyn_stack(
        move || {
            state
                .expanded
                .with(|expanded| tree::visible_entries(state.tree.get().as_ref(), expanded))
        },
        |e| e.path.clone(),
        move |e| tree_row(state, e, on_open.clone()),
    )
    .style(|s| s.flex_col().width_full().padding_vert(4.0))
    .scroll()
    .style(|s| s.size_full().min_height(0.0))
}

fn tree_row(state: ReviewState, entry: TreeEntry, on_open: OpenFile) -> impl IntoView {
    let click_path = entry.path.clone();
    let status_path = entry.path.clone();
    let (name_path, row_path) = (entry.path.clone(), entry.path.clone());
    let chevron_path = entry.path.clone();
    let is_expanded = move || state.expanded.with(|e| e.contains(&chevron_path));
    let is_dir = entry.is_dir;
    let depth = entry.depth;
    let name = entry
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&entry.path)
        .to_string();

    let active_name = move || state.active.with(|a| a.as_deref() == Some(name_path.as_str()));
    let active_row = move || state.active.with(|a| a.as_deref() == Some(row_path.as_str()));
    // A file the pull request touched carries the same letter here as in the
    // changed list, so the diff is visible while browsing the whole tree.
    let status = move || state.status_of(&status_path);

    Stack::horizontal((
        Empty::new().style(move |s| s.width(depth as f64 * 12.0).flex_shrink(0.0_f32)),
        // A chevron drawn rather than typed: the arrow characters are not in
        // the UI font and rendered as missing-glyph boxes.
        Container::new(dyn_container(
            move || is_dir.then(&is_expanded),
            move |open| match open {
                Some(true) => sidebar_glyph(SidebarIcon::ChevronDown, theme::FG_SUBTLE).into_any(),
                Some(false) => sidebar_glyph(SidebarIcon::ChevronRight, theme::FG_SUBTLE).into_any(),
                None => Empty::new().into_any(),
            },
        ))
        .style(|s| {
            s.size(14.0, 14.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .margin_right(3.0)
        }),
        Label::derived(move || name.clone()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(if is_dir { theme::FG_MUTED } else { theme::FG })
                .font_weight(if active_name() { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .text_ellipsis()
        }),
        dyn_container(status, move |s| match s {
            Some(status) => status_letter(status).into_any(),
            None => Empty::new().into_any(),
        }),
    ))
    .on_event_stop(el::Click, move |_, _| {
        if is_dir {
            state.expanded.update(|expanded| tree::toggle(expanded, &click_path));
        } else {
            on_open(&click_path);
        }
    })
    .style(move |s| {
        let base = crate::ui::widgets::clickable(s)
            .height(26.0)
            .width_full()
            .items_center()
            .padding_horiz(10.0);
        let base = base.hover(|s| s.background(theme::CANVAS_HOVER));
        if active_row() {
            base.background(theme::ACCENT_SUBTLE)
        } else {
            base
        }
    })
}
