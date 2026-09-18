//! Item rows: a pull request, an issue, or a branch of the reader's own,
//! under its project in the sidebar.
//!
//! One row draws every shape an item can have. The glyph and the reference
//! line come from which parts the item has, never from a stored kind.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{Container, Decorators, Empty, Label, Stack, dyn_container};
use perch_github::{Item, ItemSubject, Project, SubjectState};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{clickable, icon_button, sidebar_glyph};

/// Opening an item. Shared by every row.
pub type OpenItem = Rc<dyn Fn(&Item)>;

/// The state glyph an item shows: the pull request or issue state icon,
/// or a branch for a checkout with no subject.
pub fn state_of(item: &Item) -> Option<SubjectState> {
    match &item.subject {
        Some(ItemSubject::PullRequest(pr)) => Some(if pr.merged {
            SubjectState::PrMerged
        } else if !pr.is_open() {
            SubjectState::PrClosed
        } else if pr.draft {
            SubjectState::PrDraft
        } else {
            SubjectState::PrOpen
        }),
        Some(ItemSubject::Issue(issue)) => Some(if issue.is_open() {
            SubjectState::IssueOpen
        } else if issue.state_reason.as_deref() == Some("not_planned") {
            SubjectState::IssueNotPlanned
        } else {
            SubjectState::IssueClosed
        }),
        None => None,
    }
}

/// `owner/repo #57 · branch`, `owner/repo #42`, or `owner/repo · branch`.
/// Under a date heading the project name stands in for the repository.
pub fn reference(item: &Item, project: Option<&Project>, by_date: bool) -> String {
    let place = match (by_date, project) {
        (true, Some(p)) => p.name.clone(),
        _ => item.repo_full_name.clone(),
    };
    match (&item.subject, item.branch()) {
        (Some(ItemSubject::PullRequest(pr)), _) => {
            format!("{place} #{} \u{00b7} {}", pr.number, pr.head_ref)
        }
        (Some(ItemSubject::Issue(issue)), _) => format!("{place} #{}", issue.number),
        (None, Some(branch)) => format!("{place} \u{00b7} {branch}"),
        (None, None) => place,
    }
}

/// What the attention reason says on a Priority row.
pub fn reason_label(reason: &str) -> &str {
    match reason {
        "review_requested" => "review requested",
        "mention" => "mentioned",
        "team_mention" => "team mentioned",
        "comment" => "commented",
        "assign" => "assigned",
        "author" => "activity",
        "state_change" => "state changed",
        "ci_activity" => "CI",
        "ci_failed" => "CI failed",
        "manual" => "marked unread",
        "subscribed" => "activity",
        other => other,
    }
}

/// One item as the sidebar draws it.
pub fn item_row(
    state: AppState,
    backend: Rc<Backend>,
    item: Item,
    project: Option<Project>,
    by_date: bool,
    on_open: OpenItem,
) -> impl IntoView {
    let hover_id = item.id.clone();
    let leave_id = item.id.clone();
    let open_target = item.clone();
    let selected = {
        let id = item.id.clone();
        move || state.open_item.with(|o| o.as_deref() == Some(id.as_str()))
    };
    let archived = item.is_archived();
    let attention = item.needs_attention();
    let title = item.title().to_string();
    let reference = reference(&item, project.as_ref(), by_date);

    let content = Stack::horizontal((
        glyph(&item),
        Stack::vertical((
            Label::derived(move || title.clone()).style(move |s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(if attention { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL })
                    .color(if archived { theme::FG_SUBTLE } else { theme::FG })
                    .text_ellipsis()
                    .max_width_full()
            }),
            Label::derived(move || reference.clone()).style(|s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(theme::FG_SUBTLE)
                    .text_ellipsis()
                    .max_width_full()
                    .margin_top(1.0)
            }),
        ))
        .style(|s| s.flex_col().flex_grow(1.0_f32).min_width(0.0).justify_center()),
        row_actions(state, backend, item.clone()),
        attention_dot(attention),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0));

    Container::new(content)
        .on_event_stop(el::Click, move |_, _| on_open(&open_target))
        .on_event_cont(el::PointerEnter, move |_, _| state.hovered_item.set(Some(hover_id.clone())))
        .on_event_cont(el::PointerLeave, move |_, _| {
            state.hovered_item.update(|h| {
                if h.as_deref() == Some(leave_id.as_str()) {
                    *h = None;
                }
            })
        })
        .style(move |s| {
            let base = clickable(s)
                .width_full()
                .height(44.0)
                .items_center()
                .padding_left(10.0)
                .padding_right(6.0)
                .margin_horiz(6.0)
                .border_radius(theme::RADIUS);
            if selected() {
                base.background(theme::ACCENT_SUBTLE)
            } else {
                base.hover(|s| s.background(theme::CANVAS_HOVER))
            }
        })
}

fn glyph(item: &Item) -> impl IntoView {
    let state = state_of(item);
    let muted = item.is_archived();
    Container::new(match state {
        Some(state) => floem::views::svg(move || icons::state_icon(state))
            .style(move |s| {
                s.size(16.0, 16.0)
                    .color(if muted { theme::FG_SUBTLE } else { icons::state_color(state) })
            })
            .into_any(),
        None => sidebar_glyph(SidebarIcon::Branch, if muted { theme::FG_SUBTLE } else { theme::FG_MUTED })
            .into_any(),
    })
    .style(|s| s.size(16.0, 16.0).margin_right(10.0).flex_shrink(0.0_f32))
}

/// The accent dot at the right edge of a row that needs looking at.
fn attention_dot(attention: bool) -> impl IntoView {
    Container::new(if attention {
        Empty::new()
            .style(|s| s.size(7.0, 7.0).border_radius(4.0).background(theme::ACCENT))
            .into_any()
    } else {
        Empty::new().into_any()
    })
    .style(|s| s.width(12.0).items_center().justify_center().flex_shrink(0.0_f32))
}

/// Save and Done, on hover, as the inbox rows have.
fn row_actions(state: AppState, backend: Rc<Backend>, item: Item) -> impl IntoView {
    let over_a = item.id.clone();
    let hovering_a = move || state.hovered_item.with(|h| h.as_deref() == Some(over_a.as_str()));
    let over_b = item.id.clone();
    let hovering_b = move || state.hovered_item.with(|h| h.as_deref() == Some(over_b.as_str()));
    let saved = item.saved;
    let done = item.is_done();

    let save_id = item.id.clone();
    let save_backend = backend.clone();
    let done_id = item.id.clone();

    Stack::horizontal((
        dyn_container(
            move || hovering_a() || saved,
            move |show| {
                if !show {
                    return Empty::new().into_any();
                }
                let id = save_id.clone();
                let backend = save_backend.clone();
                icon_button(
                    move || if saved { SidebarIcon::BookmarkFilled } else { SidebarIcon::Bookmark },
                    move || if saved { theme::ACCENT } else { theme::FG_SUBTLE },
                    move || backend.mark_item(&id, Some(!saved), None),
                )
                .into_any()
            },
        )
        .style(|s| s.size(24.0, 24.0)),
        dyn_container(hovering_b, move |show| {
            if !show {
                return Empty::new().into_any();
            }
            let id = done_id.clone();
            let backend = backend.clone();
            icon_button(
                move || if done { SidebarIcon::Undo } else { SidebarIcon::Check },
                || theme::FG_SUBTLE,
                move || backend.mark_item(&id, None, Some(!done)),
            )
            .into_any()
        })
        .style(|s| s.size(24.0, 24.0)),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32))
}

/// A section heading in the sidebar: a name and a count in the muted colour.
pub fn section_header(title: String, count: usize) -> impl IntoView {
    Stack::horizontal((
        Label::derived(move || title.clone()).style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(theme::FG_SUBTLE)
                .text_ellipsis()
                .min_width(0.0)
                .selectable(false)
        }),
        Label::derived(move || if count == 0 { String::new() } else { count.to_string() }).style(|s| {
            s.font_size(theme::TEXT_SECTION)
                .color(theme::FG_SUBTLE)
                .margin_left(6.0)
                .selectable(false)
        }),
    ))
    .style(|s| {
        s.items_center()
            .width_full()
            .padding_horiz(16.0)
            .padding_top(14.0)
            .padding_bottom(4.0)
            .hover(|s| s.cursor(CursorStyle::Default))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::ItemCheckout;

    fn item() -> Item {
        Item::new("perch", "criccomini/perch", "2026-09-18T10:00:00Z".parse().unwrap())
    }

    #[test]
    fn the_reference_line_follows_the_shape() {
        let mut i = item();
        i.checkout = Some(ItemCheckout {
            branch: "fix-thing".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/x".into(),
            owned: false,
            on_disk: true,
        });
        assert_eq!(reference(&i, None, false), "criccomini/perch · fix-thing");
        let project = Project::implicit("perch");
        assert_eq!(reference(&i, Some(&project), true), "perch · fix-thing");
        assert_eq!(state_of(&i), None);
    }

    #[test]
    fn reasons_read_as_words() {
        assert_eq!(reason_label("review_requested"), "review requested");
        assert_eq!(reason_label("manual"), "marked unread");
        assert_eq!(reason_label("odd"), "odd");
    }
}
