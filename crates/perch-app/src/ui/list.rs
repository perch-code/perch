//! The notification list: grouped cards of rows.

use floem::prelude::*;
use floem::text::FontWeight;
use floem::views::{
    ClipExt, Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, dyn_stack,
};

use crate::backend::Status;
use crate::filter::{Group, Section, View};
use crate::icons::SidebarIcon;
use crate::state::{AppState, Checked};
use crate::theme;
use crate::ui::row::row;
use crate::ui::widgets::{avatar, centered_message, checkbox, count_badge, hairline, text_button};

/// Opening a notification. Shared by every row.
pub type OpenThread = std::rc::Rc<dyn Fn(&str, u64, &str)>;

/// Which flag an action sets, and to what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkAction {
    Saved(bool),
    Done(bool),
}

/// Setting one flag across a batch of threads.
///
/// Shared by the row buttons, the keyboard, the selection bar and the group
/// headers, so every one of them costs the backend a single write.
pub type OnMark = std::rc::Rc<dyn Fn(Vec<String>, MarkAction)>;

pub fn list(state: AppState, on_open: OpenThread, on_mark: OnMark) -> impl IntoView {
    dyn_container(
        move || ListMode::of(state),
        move |mode| match mode {
            ListMode::NoToken => token_help(state).into_any(),
            ListMode::Loading => centered_message(
                SidebarIcon::Refresh,
                || "Loading notifications".to_string(),
                || "Fetching your inbox from GitHub.".to_string(),
            )
            .into_any(),
            ListMode::Empty(view) => {
                let (icon, title, detail) = empty_state(view);
                centered_message(icon, move || title.to_string(), move || detail.to_string())
                    .into_any()
            }
            ListMode::NoMatches => centered_message(
                SidebarIcon::Search,
                || "No notifications match".to_string(),
                move || {
                    if state.filters().is_narrowed() {
                        "Try clearing a filter or the search box.".to_string()
                    } else {
                        "Nothing to show right now.".to_string()
                    }
                },
            )
            .into_any(),
            ListMode::Rows => rows(state, on_open.clone(), on_mark.clone()).into_any(),
        },
    )
    .style(|s| s.size_full().min_height(0.0).background(theme::CANVAS_SUBTLE))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListMode {
    NoToken,
    Loading,
    /// The open view holds nothing at all, which each view says its own way.
    Empty(View),
    NoMatches,
    Rows,
}

/// What an empty view says for itself.
fn empty_state(view: View) -> (SidebarIcon, &'static str, &'static str) {
    match view {
        View::Inbox => (
            SidebarIcon::Inbox,
            "You are all caught up",
            "No notifications in your inbox.",
        ),
        View::Saved => (
            SidebarIcon::Bookmark,
            "Nothing saved",
            "Bookmark a notification to keep it here.",
        ),
        View::Done => (
            SidebarIcon::Check,
            "Nothing done yet",
            "Notifications you finish with land here.",
        ),
    }
}

impl ListMode {
    fn of(state: AppState) -> Self {
        if matches!(state.status.get(), Status::NoToken { .. }) {
            return ListMode::NoToken;
        }
        // Only the inbox can be loading. Saved and Done are Perch's own, so
        // they are never waiting on anything: they paint before the network
        // answers, and they paint when it never answers at all. Holding them
        // behind the inbox's flag showed a spinner over rows that were
        // already in hand, and showed it forever when the cached inbox was
        // empty but the marks were not.
        let view = state.view.get();
        if state.view_total() == 0 {
            return if view == View::Inbox && !state.loaded.get() {
                ListMode::Loading
            } else {
                ListMode::Empty(view)
            };
        }
        if state.visible().is_empty() {
            return ListMode::NoMatches;
        }
        ListMode::Rows
    }
}

fn rows(state: AppState, on_open: OpenThread, on_mark: OnMark) -> impl IntoView {
    Stack::vertical((
        selection_bar(state, on_mark.clone()),
        dyn_stack(
            move || state.sections(),
            |section| section.key.clone(),
            move |section| section_card(state, section, on_open.clone(), on_mark.clone()),
        )
        .style(|s| s.flex_col().width_full().gap(16.0).padding(16.0))
        .scroll()
        .style(|s| s.size_full().min_height(0.0)),
    ))
    .style(|s| s.flex_col().size_full().min_height(0.0))
}

/// Select all, and what to do with the selection.
///
/// Always on screen above the rows, so checking the first box does not shove
/// the whole list down by the height of a bar.
fn selection_bar(state: AppState, on_mark: OnMark) -> impl IntoView {
    let count = move || state.selection_len();

    let label = move || match count() {
        0 => "Select all".to_string(),
        1 => "1 selected".to_string(),
        n => format!("{n} selected"),
    };

    // Select all means every row the filters show, not every row on screen.
    let toggle = move |_extend: bool| {
        if state.selection_state() == Checked::Full {
            state.clear_selection();
        } else {
            state.select_all_visible();
        }
    };

    Stack::horizontal((
        checkbox(move || state.selection_state(), toggle),
        Label::derived(label).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::FG_MUTED)
                .selectable(false)
                .margin_left(6.0)
                .flex_grow(1.0_f32)
        }),
        dyn_container(
            move || {
                let ids = state.selection();
                (
                    !ids.is_empty(),
                    state.view.get() == View::Saved,
                    state.would_restore(&ids),
                    state.would_unsave(&ids),
                )
            },
            move |(any, saved_view, restoring, unsaving)| {
                if !any {
                    return Empty::new().into_any();
                }
                bulk_actions(state, on_mark.clone(), saved_view, restoring, unsaving).into_any()
            },
        ),
    ))
    .style(|s| {
        s.height(44.0)
            .width_full()
            .items_center()
            .flex_shrink(0.0_f32)
            .padding_horiz(16.0)
            .background(theme::CANVAS)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// The two buttons that act on a selection.
///
/// In Saved the bookmark is the reason the reader is there, so it leads.
fn bulk_actions(
    state: AppState,
    on_mark: OnMark,
    saved_view: bool,
    restoring: bool,
    unsaving: bool,
) -> impl IntoView {
    let dismiss_mark = on_mark.clone();
    let dismiss = move || {
        let ids = state.selection();
        dismiss_mark(ids, MarkAction::Done(!restoring));
        state.clear_selection();
    };
    let save = move || {
        let ids = state.selection();
        on_mark(ids, MarkAction::Saved(!unsaving));
        state.clear_selection();
    };

    let dismiss_button = text_button(
        move || {
            if restoring {
                "Restore".to_string()
            } else {
                "Mark as done".to_string()
            }
        },
        if restoring {
            SidebarIcon::Undo
        } else {
            SidebarIcon::Check
        },
        dismiss,
    );
    let save_button = text_button(
        move || {
            if unsaving {
                "Unsave".to_string()
            } else {
                "Save".to_string()
            }
        },
        if unsaving {
            SidebarIcon::BookmarkFilled
        } else {
            SidebarIcon::Bookmark
        },
        save,
    );

    let pair = if saved_view {
        Stack::horizontal((save_button.into_any(), dismiss_button.into_any()))
    } else {
        Stack::horizontal((dismiss_button.into_any(), save_button.into_any()))
    };
    pair.style(|s| s.items_center().gap(8.0))
}

/// One repository or date group, drawn as a bordered card.
fn section_card(
    state: AppState,
    section: Section,
    on_open: OpenThread,
    on_mark: OnMark,
) -> impl IntoView {
    let grouped = state.group.get() != Group::None;
    let threads = section.threads.clone();
    let count = threads.len();

    let header = if grouped {
        section_header(state, &section, count, on_mark.clone()).into_any()
    } else {
        // Group: None is one section holding everything, which the selection
        // bar above the list already speaks for.
        Empty::new().into_any()
    };

    Stack::vertical((
        header,
        dyn_stack(
            move || threads.clone().into_iter().enumerate(),
            |(i, t)| (*i, t.id.clone()),
            move |(i, thread)| {
                Stack::vertical((
                    if i == 0 {
                        Empty::new().into_any()
                    } else {
                        hairline().into_any()
                    },
                    row(state, thread, on_open.clone(), on_mark.clone()),
                ))
                .style(|s| s.width_full())
            },
        )
        .style(|s| s.flex_col().width_full()),
    ))
    // The sizing has to go on the stack itself. `clip` wraps it, so a style
    // applied after that lands on the wrapper and leaves the stack to shrink
    // to its content: a repository with short titles ended up narrower than
    // its own card.
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

fn section_header(
    state: AppState,
    section: &Section,
    count: usize,
    on_mark: OnMark,
) -> impl IntoView {
    let title = section.title.clone();
    let seed = section.title.clone();
    let avatar_url = section.avatar_url.clone().unwrap_or_default();
    let has_avatar = section.avatar_url.is_some();

    Stack::horizontal((
        Container::new(if has_avatar {
            avatar(state, move || avatar_url.clone(), 18.0, seed).into_any()
        } else {
            Empty::new().into_any()
        })
        .style(move |s| {
            if has_avatar {
                s.margin_right(9.0).flex_shrink(0.0_f32)
            } else {
                s.flex_shrink(0.0_f32)
            }
        }),
        Label::derived(move || title.clone()).style(|s| {
            s.font_size(theme::TEXT_BODY)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG)
                .text_ellipsis()
                .min_width(0.0)
                .margin_right(8.0)
        }),
        count_badge(move || count, false),
        // Everything in this group, whatever is checked elsewhere.
        Container::new(section_action(state, section, on_mark))
            .style(|s| s.flex_grow(1.0_f32).justify_end()),
    ))
    .style(|s| {
        s.height(40.0)
            .width_full()
            .items_center()
            .padding_horiz(14.0)
            .background(theme::CANVAS_SUBTLE)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    })
}

/// Dismiss a whole repository, or a whole day, in one go.
///
/// Deliberately ignores the selection: this button says what it acts on, and
/// it is the rows underneath it.
fn section_action(state: AppState, section: &Section, on_mark: OnMark) -> impl IntoView {
    let ids: Vec<String> = section.threads.iter().map(|t| t.id.clone()).collect();
    let restoring = state.would_restore(&ids);

    let act = move || {
        on_mark(ids.clone(), MarkAction::Done(!restoring));
        // The rows are about to leave, and with them any check on one.
        state.clear_selection();
    };

    text_button(
        move || {
            if restoring {
                "Restore".to_string()
            } else {
                "Mark as done".to_string()
            }
        },
        if restoring {
            SidebarIcon::Undo
        } else {
            SidebarIcon::Check
        },
        act,
    )
    .style(|s| s.height(24.0).padding_horiz(8.0))
}

/// Shown when no token resolved, in place of the list.
fn token_help(state: AppState) -> impl IntoView {
    let detail = move || match state.status.get() {
        Status::NoToken { detail } => detail,
        _ => String::new(),
    };

    Container::new(
        Stack::vertical((
            Container::new(crate::ui::widgets::sidebar_glyph(
                SidebarIcon::Warning,
                theme::FG_SUBTLE,
            ))
            .style(|s| {
                s.size(48.0, 48.0)
                    .border_radius(24.0)
                    .background(theme::CANVAS_SUBTLE)
                    .items_center()
                    .justify_center()
                    .margin_bottom(14.0)
            }),
            Label::derived(|| "No GitHub token found").style(|s| {
                s.font_size(15.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
                    .margin_bottom(6.0)
            }),
            Label::derived(detail).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::FG_MUTED)
                    .max_width(460.0)
                    .margin_bottom(16.0)
            }),
            Stack::vertical((
                code_line("export GITHUB_TOKEN=ghp_..."),
                code_line("gh auth login"),
            ))
            .style(|s| s.gap(6.0)),
            Label::derived(|| "The token needs the notifications scope.").style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::FG_SUBTLE)
                    .margin_top(14.0)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .style(|s| s.size_full().items_center().justify_center().padding(40.0))
}

fn code_line(text: &'static str) -> impl IntoView {
    Label::derived(move || text).style(|s| {
        s.font_family("SF Mono, Menlo, monospace".to_string())
            .font_size(theme::TEXT_META)
            .color(theme::FG)
            .background(theme::CANVAS_SUBTLE)
            .border(1.0)
            .border_color(theme::BORDER_MUTED)
            .border_radius(theme::RADIUS)
            .padding_horiz(10.0)
            .padding_vert(5.0)
    })
}
