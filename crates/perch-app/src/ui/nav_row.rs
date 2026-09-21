//! The shape every row in the drawer has, as Codex draws its rows.
//!
//! A glyph at the left, then two lines. The first is the title, with a
//! compact age at its right end ("1d", "2w"), which the row's controls
//! stand in for while the pointer is over it. The second is the byline:
//! a small avatar, the repository, then the number when the row has one,
//! each cut short with an ellipsis as the drawer narrows, then a pull
//! request's chips, and at its right end the churn: "+658 -0" on a pull
//! request or a branch.
//!
//! Item rows and notification rows share these pieces so the three
//! scopes read as one list.

use floem::menu::Menu;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::{CursorStyle, Style};
use floem::text::FontWeight;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use jiff::Timestamp;
use perch_github::{CheckStatus, ReviewDecision};

use crate::icons::SidebarIcon;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::widgets::{avatar, clickable, sidebar_glyph};

/// The first line is as tall as a control, so the controls that stand in
/// for the age on hover do not move the byline under them.
pub const TITLE_LINE_HEIGHT: f64 = 26.0;
/// The byline's avatar.
pub const AVATAR: f64 = 16.0;

/// The row itself: the button the whole row is, at the drawer's inset.
///
/// A selected row is washed and stays washed under the pointer, a little
/// darker; any other row lights only under the pointer.
pub fn row_style(s: Style, selected: bool) -> Style {
    let base = clickable(s)
        .width_full()
        .height(theme::ROW_HEIGHT)
        .items_center()
        .padding_left(12.0)
        .padding_right(13.0)
        .border_radius(theme::NAV_ROW_RADIUS);
    if selected {
        base.background(theme::NAV_SELECTED)
            .hover(|s| s.background(theme::NAV_SELECTED_PRESSED))
    } else {
        base.hover(|s| s.background(theme::NAV_HOVER))
    }
}

/// The gutter either side of a row, between it and the drawer's edges.
pub fn row_gutter(s: Style) -> Style {
    s.width_full().padding_horiz(24.0)
}

/// The list the rows sit in: the padding above the first and below the
/// last, and the hairline between them.
pub fn list_style(s: Style) -> Style {
    s.flex_col()
        .width_full()
        .gap(1.0)
        .padding_top(18.0)
        .padding_bottom(16.0)
}

/// The box the glyph is centred in, with its gap to the lines.
pub fn glyph_box(glyph: impl IntoView + 'static) -> impl IntoView {
    Container::new(glyph).style(|s| {
        s.size(20.0, 20.0)
            .items_center()
            .justify_center()
            .margin_right(10.0)
            .flex_shrink(0.0_f32)
    })
}

/// The two lines beside the glyph. The title takes the width the trailing
/// views leave it; the byline takes the whole width.
pub fn lines(
    title: impl IntoView + 'static,
    trailing: impl IntoView + 'static,
    byline: impl IntoView + 'static,
) -> impl IntoView {
    Stack::vertical((
        Stack::horizontal((
            Container::new(title).style(|s| s.flex_grow(1.0_f32).min_width(0.0).items_center()),
            trailing,
        ))
        .style(|s| {
            s.items_center()
                .width_full()
                .min_width(0.0)
                .height(TITLE_LINE_HEIGHT)
        }),
        byline,
    ))
    .style(|s| {
        s.flex_col()
            .flex_grow(1.0_f32)
            .min_width(0.0)
            .justify_center()
    })
}

/// The title, in one style, so a renamed branch and an unread
/// notification differ only where they mean to.
pub fn title_label(text: String, muted: bool, bold: bool) -> impl IntoView {
    Label::derived(move || text.clone()).style(move |s| {
        s.font_size(theme::TEXT_TITLE)
            .font_weight(if bold {
                FontWeight::SEMI_BOLD
            } else {
                FontWeight::NORMAL
            })
            .color(if muted {
                theme::NAV_SUBTLE
            } else {
                theme::NAV_FG
            })
            .text_ellipsis()
            .max_width_full()
    })
}

/// The second line: the avatar, the repository, the reference, and the
/// figures at the right end.
///
/// The repository and the reference each give way with an ellipsis as
/// the drawer narrows; the figures never do. `after` sits between the
/// reference and the figures: the `chips` on a pull request. The figures
/// are the `churn`, on a pull request or a branch, and nothing on an
/// issue or a row that is neither.
pub fn byline(
    state: AppState,
    avatar_url: impl Fn() -> String + 'static,
    seed: String,
    place: String,
    reference: String,
    after: impl IntoView + 'static,
    figures: impl IntoView + 'static,
) -> impl IntoView {
    let muted_text = |s: Style| {
        s.font_size(theme::TEXT_META)
            .color(theme::NAV_MUTED)
            .text_ellipsis()
            .min_width(0.0)
            .flex_shrink(1.0_f32)
    };
    Stack::horizontal((
        Container::new(avatar(state, avatar_url, AVATAR, seed))
            .debug_name("row avatar")
            .style(|s| s.margin_right(6.0).flex_shrink(0.0_f32)),
        Label::derived(move || place.clone()).style(muted_text),
        if reference.is_empty() {
            Empty::new().into_any()
        } else {
            Label::derived(move || reference.clone())
                .style(move |s| muted_text(s).margin_left(10.0))
                .into_any()
        },
        after,
        Empty::new().style(|s| s.flex_grow(1.0_f32).min_width(8.0)),
        Container::new(figures)
            .debug_name("row figures")
            .style(|s| s.items_center().flex_shrink(0.0_f32)),
    ))
    .debug_name("row byline")
    .style(|s| s.items_center().width_full().min_width(0.0).margin_top(3.0))
}

/// "1d", at the title's right end, following the app's clock.
pub fn age(state: AppState, at: Timestamp) -> impl IntoView {
    Label::derived(move || timefmt::short(at, state.now.get()))
        .debug_name("row age")
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::NAV_MUTED)
                .flex_shrink(0.0_f32)
                .margin_left(8.0)
        })
}

/// A small glyph beside the age: the pin, the bookmark.
pub fn mark(glyph: SidebarIcon, color: floem::peniko::Color) -> impl IntoView {
    sidebar_glyph(glyph, color).style(|s| s.size(11.0, 11.0).flex_shrink(0.0_f32).margin_left(6.0))
}

/// "#2085", or nothing: the reference a byline carries is the number
/// when the row has one.
pub fn reference(number: Option<u64>) -> String {
    number
        .map(|number| format!("#{number}"))
        .unwrap_or_default()
}

/// The CI chip and the review pill after a pull request's reference,
/// while the filter menu shows PR status. Nothing on a row with neither.
pub fn chips(
    state: AppState,
    checks: Option<CheckStatus>,
    review: Option<ReviewDecision>,
) -> impl IntoView {
    dyn_container(
        move || {
            state.item_filters.with(|f| f.show_pr_status) && (checks.is_some() || review.is_some())
        },
        move |show| {
            if !show {
                return Empty::new().into_any();
            }
            Stack::horizontal((
                match checks {
                    Some(CheckStatus::Success) => sidebar_glyph(SidebarIcon::Check, theme::OPEN)
                        .style(|s| s.size(11.0, 11.0))
                        .into_any(),
                    Some(CheckStatus::Failure) => sidebar_glyph(SidebarIcon::Close, theme::CLOSED)
                        .style(|s| s.size(11.0, 11.0))
                        .into_any(),
                    Some(CheckStatus::Pending) => Empty::new()
                        .style(|s| {
                            s.size(7.0, 7.0)
                                .border_radius(4.0)
                                .background(theme::DRAFT_YELLOW)
                        })
                        .into_any(),
                    None => Empty::new().into_any(),
                },
                match review {
                    Some(r) => Label::derived(move || r.label().to_string())
                        .style(move |s| {
                            let color = match r {
                                ReviewDecision::Approved => theme::OPEN,
                                ReviewDecision::ChangesRequested => theme::CLOSED,
                                ReviewDecision::ReviewRequested => theme::FG_MUTED,
                            };
                            s.font_size(theme::TEXT_SECTION)
                                .color(color)
                                .border(1.0)
                                .border_color(color.with_alpha(0.4))
                                // A pill, at the radius the review's chips use.
                                .border_radius(theme::CHIP_HEIGHT / 2.0)
                                .padding_horiz(5.0)
                                .margin_left(5.0)
                                .selectable(false)
                        })
                        .into_any(),
                    None => Empty::new().into_any(),
                },
            ))
            .style(|s| s.items_center().margin_left(6.0).flex_shrink(0.0_f32))
            .into_any()
        },
    )
}

/// "+1,192 -0", the way Codex writes a branch's churn: the additions in
/// green, the deletions in red. `None` draws nothing, for a row that has
/// no churn to say.
pub fn churn(churn: Option<(u32, u32)>) -> impl IntoView {
    match churn {
        Some((added, deleted)) => churn_figures(added, deleted).into_any(),
        None => Empty::new().into_any(),
    }
}

fn churn_figures(added: u32, deleted: u32) -> impl IntoView {
    let (added, deleted) = (
        format!("+{}", thousands(added)),
        format!("-{}", thousands(deleted)),
    );
    Stack::horizontal((
        Label::derived(move || added.clone()).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::NAV_ADDED)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || deleted.clone()).style(|s| {
            s.font_size(theme::TEXT_META)
                .color(theme::NAV_DELETED)
                .flex_shrink(0.0_f32)
                .margin_left(5.0)
        }),
    ))
    .style(|s| s.items_center().flex_shrink(0.0_f32))
}

/// 1192 as "1,192".
fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The `...` button: the row's menu.
pub fn more_button(menu: impl Fn() -> Menu + 'static) -> impl IntoView {
    Container::new(
        sidebar_glyph(SidebarIcon::Ellipsis, theme::FG_SUBTLE)
            // The menu looks only at the deepest view under the pointer, so
            // the glyph must not be it.
            .style(|s| s.pointer_events_none()),
    )
    .popout_menu(menu)
    // The row underneath is a button too.
    .on_event_stop(el::Click, |_, _| {})
    .style(|s| {
        clickable(s)
            .size(26.0, 26.0)
            .items_center()
            .justify_center()
            .border_radius(13.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_is_the_number_or_nothing() {
        assert_eq!(reference(Some(2085)), "#2085");
        assert_eq!(reference(None), "");
    }

    #[test]
    fn a_comma_goes_every_three_digits_as_codex_writes_a_count() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(658), "658");
        assert_eq!(thousands(1192), "1,192");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }
}
