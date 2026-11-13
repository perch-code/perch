//! The shape every row in the drawer has, as Codex draws its rows.
//!
//! A glyph at the left, then two lines. The first is the title, with a
//! compact age at its right end ("1d", "2w"), which the row's controls
//! stand in for while the pointer is over it. The second is the byline:
//! a small stack of faces, the repository, then the number when the row
//! has one, each cut short with an ellipsis as the drawer narrows, then a
//! pull request's status glyphs, and at its right end the churn: "+658 -0" on a pull
//! request or a branch.
//!
//! Item rows and notification rows share these pieces so the three
//! scopes read as one list.

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet};
use floem::style::{CursorStyle, Style};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use jiff::Timestamp;
use perch_github::{CheckStatus, ReviewDecision};

use crate::icons::SidebarIcon;
use crate::review::cards::tip;
use crate::state::AppState;
use crate::theme;
use crate::timefmt;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::widgets::{avatar, avatar_radius, clickable, sidebar_glyph};

/// The first line is as tall as a control, so the controls that stand in
/// for the age on hover do not move the byline under them.
pub const TITLE_LINE_HEIGHT: f64 = 26.0;
/// A face in the byline's stack.
pub const AVATAR: f64 = 16.0;
/// How far each face behind starts from the one in front of it.
pub const FACE_STEP: f64 = 10.0;
/// The ring in the row's own colour that cuts a face out of the one
/// behind it, as GitHub's stacks do.
const FACE_RING: f64 = 1.5;

floem::style_class!(
    /// The ring around a face in a byline's stack. The row paints it in
    /// its own wash, so the ring follows the row's hover and selection.
    pub FaceRing
);

/// The row itself: the button the whole row is, at the drawer's inset.
///
/// A selected row is washed and stays washed under the pointer, a little
/// darker; any other row lights only under the pointer.
pub fn row_style(s: Style, selected: bool) -> Style {
    let base = clickable(s)
        .class(FaceRing, |s| s.background(theme::nav_canvas()))
        .width_full()
        .height(theme::ROW_HEIGHT)
        .items_center()
        .padding_left(12.0)
        .padding_right(13.0)
        .border_radius(theme::NAV_ROW_RADIUS);
    if selected {
        base.background(theme::nav_selected())
            .class(FaceRing, |s| s.background(theme::nav_selected()))
            .hover(|s| {
                s.background(theme::nav_selected_pressed())
                    .class(FaceRing, |s| s.background(theme::nav_selected_pressed()))
            })
    } else {
        base.hover(|s| {
            s.background(theme::nav_hover())
                .class(FaceRing, |s| s.background(theme::nav_hover()))
        })
    }
}

/// The gutter either side of a row, between it and the drawer's edges.
pub fn row_gutter(s: Style) -> Style {
    s.width_full().padding_horiz(theme::NAV_GUTTER)
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
///
/// An unread row's title is Medium in the full foreground. A read one
/// steps back on both counts, to Regular in `NAV_READ`, because the
/// weight alone is a tenth of a pixel of stem at this size and the two
/// read as the same title. `unread` is read with the style, so a row
/// whose notification is read while it is on screen steps back without
/// being rebuilt.
pub fn title_label(
    text: String,
    muted: bool,
    unread: impl Fn() -> bool + 'static,
) -> impl IntoView {
    Label::derived(move || text.clone()).style(move |s| {
        let unread = unread();
        s.font_size(theme::TEXT_TITLE)
            .font_weight(if unread {
                theme::weight_emphasis()
            } else {
                theme::weight_read()
            })
            .color(if muted {
                theme::nav_subtle()
            } else if unread {
                theme::nav_fg()
            } else {
                theme::nav_read()
            })
            .text_ellipsis()
            .max_width_full()
    })
}

/// Someone in a byline's stack: where their picture comes from, and the
/// name whose initial stands in until it arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    pub url: String,
    pub seed: String,
}

impl Face {
    pub fn new(url: impl Into<String>, seed: impl Into<String>) -> Self {
        Face {
            url: url.into(),
            seed: seed.into(),
        }
    }
}

/// The faces of who acted last, most recent first, as a look found them:
/// the stack GitHub's inbox draws.
pub fn participant_faces(participants: &[perch_github::Participant]) -> Vec<Face> {
    participants
        .iter()
        .map(|p| Face::new(p.avatar_url.clone(), p.login.clone()))
        .collect()
}

/// The faces, most recent first, overlapped as GitHub's inbox overlaps
/// them: the most recent at the right and in front, the rest leftward and
/// behind. Each wears a ring in the row's colour that stands out of the
/// layout, so the stack's box is the faces alone. Rebuilt only when the
/// faces change, as a look lands or the viewer arrives.
fn face_stack(state: AppState, faces: impl Fn() -> Vec<Face> + 'static) -> impl IntoView {
    let faces = Memo::new(move |_| faces());
    dyn_container(
        move || faces.get(),
        move |faces| {
            Stack::horizontal_from_iter(faces.into_iter().rev().enumerate().map(
                move |(i, face)| {
                    let radius = avatar_radius(AVATAR, &face.url) + FACE_RING;
                    let url = face.url;
                    Container::new(avatar(state, move || url.clone(), AVATAR, face.seed))
                        .debug_name(format!("row face:{i}"))
                        .class(FaceRing)
                        .style(move |s| {
                            s.padding(FACE_RING)
                                .margin(-FACE_RING)
                                .border_radius(radius)
                                .apply_if(i > 0, |s| s.margin_left(FACE_STEP - AVATAR - FACE_RING))
                        })
                },
            ))
            .into_any()
        },
    )
}

/// The second line: the faces, the repository, the reference, and the
/// figures at the right end.
///
/// The repository and the reference each give way with an ellipsis as
/// the drawer narrows; the figures never do. `after` sits between the
/// reference and the figures: the `chips` on a pull request. The figures
/// are the `churn`, on a pull request or a branch, and nothing on an
/// issue or a row that is neither.
pub fn byline(
    state: AppState,
    faces: impl Fn() -> Vec<Face> + 'static,
    place: String,
    reference: String,
    after: impl IntoView + 'static,
    figures: impl IntoView + 'static,
) -> impl IntoView {
    let muted_text = |s: Style| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::nav_muted())
            .text_ellipsis()
            .min_width(0.0)
            .flex_shrink(1.0_f32)
    };
    Stack::horizontal((
        Container::new(face_stack(state, faces))
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

/// A few words after the chips, in the byline's size: how a worktree
/// stands against its pull request, or what an agent last did there.
/// Nothing when there's nothing to say.
pub fn note(text: Option<String>, color: impl theme::Ink) -> impl IntoView {
    match text {
        Some(text) => Label::derived(move || text.clone())
            .debug_name("row note")
            .style(move |s| {
                s.font_size(theme::TEXT_BADGE)
                    .color(color())
                    .margin_left(8.0)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_shrink(1.0_f32)
            })
            .into_any(),
        None => Empty::new().into_any(),
    }
}

/// "1d", at the title's right end, following the app's clock.
pub fn age(state: AppState, at: Timestamp) -> impl IntoView {
    // A memo, so the label is set again only when its text changes, not
    // on every tick of the clock.
    let text = Memo::new(move |_| timefmt::short(at, state.now.get()));
    Label::derived(move || text.get())
        .debug_name("row age")
        .style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::nav_muted())
                .flex_shrink(0.0_f32)
                .margin_left(8.0)
        })
}

/// A small glyph beside the age: the pin, the bookmark.
pub fn mark(glyph: SidebarIcon, color: impl theme::Ink) -> impl IntoView {
    sidebar_glyph(glyph, color).style(|s| s.size(11.0, 11.0).flex_shrink(0.0_f32).margin_left(6.0))
}

/// The unread reminder, before the age, in GitHub's blue. Out of the
/// layout, not just invisible, while `unread` says there is none, and
/// read with the style as the title's weight is.
pub fn unread_dot(unread: impl Fn() -> bool + 'static) -> impl IntoView {
    Empty::new().debug_name("unread dot").style(move |s| {
        s.size(7.0, 7.0)
            .border_radius(4.0)
            .background(theme::unread())
            .flex_shrink(0.0_f32)
            .margin_left(6.0)
            .apply_if(!unread(), |s| s.hide())
    })
}

/// "#2085", or nothing: the reference a byline carries is the number
/// when the row has one.
pub fn reference(number: Option<u64>) -> String {
    number
        .map(|number| format!("#{number}"))
        .unwrap_or_default()
}

/// A status glyph's side in the byline.
const CHIP_GLYPH: f64 = 12.0;

/// How a pull request stands, after its reference, as the glyphs its own
/// status line draws: the checks, then an eye for the review. Green is
/// well, yellow is waiting, red blocks; the words are under the pointer.
/// Nothing on a row with neither.
pub fn chips(checks: Option<CheckStatus>, review: Option<ReviewDecision>) -> impl IntoView {
    if checks.is_none() && review.is_none() {
        return Empty::new().into_any();
    }
    let glyph = |icon: SidebarIcon, ink: fn() -> Color, says: &str| {
        tip(
            sidebar_glyph(icon, ink)
                .debug_name(says.to_string())
                .style(|s| s.size(CHIP_GLYPH, CHIP_GLYPH).flex_shrink(0.0_f32)),
            says.to_string(),
        )
        .into_any()
    };
    Stack::horizontal((
        match checks {
            Some(CheckStatus::Success) => glyph(SidebarIcon::Check, theme::open, "checks passed"),
            Some(CheckStatus::Failure) => glyph(SidebarIcon::Close, theme::closed, "checks failed"),
            Some(CheckStatus::Pending) => {
                glyph(SidebarIcon::Refresh, theme::draft_yellow, "checks running")
            }
            None => Empty::new().into_any(),
        },
        match review {
            Some(r) => glyph(
                SidebarIcon::Unread,
                match r {
                    ReviewDecision::Approved => theme::open,
                    ReviewDecision::ChangesRequested => theme::closed,
                    ReviewDecision::ReviewRequested => theme::draft_yellow,
                },
                r.label(),
            ),
            None => Empty::new().into_any(),
        },
    ))
    .style(|s| {
        s.items_center()
            .gap(5.0)
            .margin_left(8.0)
            .flex_shrink(0.0_f32)
    })
    .into_any()
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
            s.font_size(theme::TEXT_BADGE)
                .color(theme::nav_added())
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || deleted.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::nav_deleted())
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
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The `...` button: the row's menu.
pub fn more_button(menu: impl Fn() -> Menu + 'static) -> impl IntoView {
    menu_button(SidebarIcon::Ellipsis, "row more", menu)
}

/// A round button at a row's end that drops `menu` down.
pub fn menu_button(
    glyph: SidebarIcon,
    name: &'static str,
    menu: impl Fn() -> Menu + 'static,
) -> impl IntoView {
    Container::new(sidebar_glyph(glyph, theme::fg_subtle).style(|s| s.pointer_events_none()))
        .dropdown(menu)
        // The row underneath is a button too.
        .on_event_stop(el::Click, |_, _| {})
        .debug_name(name)
        .style(|s| {
            clickable(s)
                .size(26.0, 26.0)
                .items_center()
                .justify_center()
                .border_radius(13.0)
                .hover(|s| s.background(theme::badge_bg()).cursor(CursorStyle::Pointer))
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
