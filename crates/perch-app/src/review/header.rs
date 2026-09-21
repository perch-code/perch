//! The bar above the item view.
//!
//! The bar is the window's title bar. It holds what is open, the switch
//! between Conversation and Files, and the four things a reader reaches
//! for from anywhere in an item: GitHub, an agent's review of the branch,
//! the item's one action (Submit review, or Start work on an issue), and
//! the whole window. Everything else about the item, its state and
//! number, branches, checks and worktree, and what GitHub says about it,
//! is `review::summary`, under Conversation's title.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::text::FontWeight;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::cards::{tip, tip_with};
use crate::review::model::{Phase, ReviewState};
use crate::review::summary::subject_state;
use crate::state::AppState;
use crate::theme;
use crate::ui::items::github_url;
use crate::ui::titlebar::title_bar;
use crate::ui::widgets::{icon_button, open_in_browser, sidebar_glyph};

/// The bar, the same for every kind of item: a pull request, an issue, or
/// a branch of the reader's own.
pub fn header(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // The bar is the window's title bar, so a press on anything in it that
    // is not a control drags the window. Its three parts let the pointer
    // through, and each control in them takes it back.
    title_bar(
        Stack::horizontal((
            leading(app, state),
            crate::review::switch::switch(state),
            trailing(app, state, backend),
        ))
        .debug_name("item bar"),
    )
    // Floem bounds z-index to sibling stacking contexts. Submit review and
    // the review launcher open from this bar, so the bar sits above the
    // item.
    .style(|s| bar_style(s).z_index(2))
}

fn bar_style(s: floem::style::Style) -> floem::style::Style {
    // The bottom border comes out of the bar's height, so a pixel of
    // padding at the top balances it: the bar's contents then centre on
    // the same line as the drawer's strip, which has no border, and the
    // drawer control stays put when the drawer shuts.
    s.height(theme::REVIEW_HEADER_HEIGHT)
        .width_full()
        .flex_shrink(0.0_f32)
        .items_center()
        .padding_top(1.0)
        .padding_horiz(theme::REVIEW_HEADER_PADDING)
        .gap(12.0)
        .background(theme::CANVAS)
        .border_bottom(1.0)
        .border_color(theme::BORDER_MUTED)
}

/// Either side of the switch. The two take the bar's room in equal
/// shares, so the switch sits at the bar's centre whatever is on either
/// side of it; the title on the left gives way first, since the controls
/// on the right hold their size.
fn side(s: floem::style::Style) -> floem::style::Style {
    s.items_center()
        .flex_grow(1.0_f32)
        .flex_basis(0.0)
        .pointer_events_none()
}

/// What is open: the lights and the drawer control while the drawer is
/// shut, then the item's state as a glyph, and its title.
fn leading(app: AppState, state: ReviewState) -> impl IntoView {
    Stack::horizontal((
        crate::ui::sidebar::lights_lane(app, theme::REVIEW_HEADER_PADDING)
            .style(move |s| s.apply_if(app.sidebar_open.get(), |s| s.hide())),
        state_glyph(state),
        Label::derived(move || title(state))
            .debug_name("item title")
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .font_weight(FontWeight::MEDIUM)
                    .color(theme::FG)
                    .text_ellipsis()
                    .min_width(0.0)
                    .margin_bottom(theme::OPTICAL_TEXT_LIFT)
            }),
    ))
    .style(|s| side(s).min_width(0.0).gap(8.0))
}

/// The pull request's or the issue's title. A branch of the reader's own
/// has no title but its name.
fn title(state: ReviewState) -> String {
    state
        .pull_request
        .with(|p| p.as_ref().map(|p| p.title.clone()))
        .or_else(|| state.issue.with(|i| i.as_ref().map(|i| i.title.clone())))
        .or_else(|| {
            state
                .item
                .with(|i| i.as_ref().map(|i| i.title().to_string()))
        })
        .unwrap_or_default()
}

/// The item's state as the sidebar's rows draw it: the pull request or
/// issue glyph in the state's colour. A branch has no state, and keeps its
/// own glyph.
fn state_glyph(state: ReviewState) -> impl IntoView {
    dyn_container(
        move || subject_state(state),
        move |s| match s {
            Some(state) => floem::views::svg(move || icons::state_icon(state))
                .style(move |s| s.size(16.0, 16.0).color(icons::state_color(state)))
                .into_any(),
            None => sidebar_glyph(SidebarIcon::Branch, theme::FG_MUTED)
                .style(|s| s.size(16.0, 16.0))
                .into_any(),
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// The four things reached for from anywhere in an item: GitHub, an
/// agent's review, the item's one action, and the whole window.
fn trailing(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::horizontal((
        github_button(state),
        launcher(app, state, backend.clone()),
        primary_action(app, state, backend),
        expand_button(app),
    ))
    .style(|s| side(s).justify_end().gap(8.0))
}

/// The agent review launcher. An issue has no files to review until
/// Start work gives it some, and then it is a branch.
fn launcher(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let reviewable = move || state.phase.get() != Phase::Issue;
    dyn_container(reviewable, move |reviewable| {
        if reviewable {
            crate::review::review_launch::control(app, state, backend.clone()).into_any()
        } else {
            Empty::new().into_any()
        }
    })
    .style(move |s| control(s).apply_if(!reviewable(), |s| s.hide()))
}

/// Open the pull request or the issue on github.com. A branch of the
/// reader's own is nowhere on GitHub, and has no button.
fn github_button(state: ReviewState) -> impl IntoView {
    let url = move || state.item.with(|i| i.as_deref().and_then(github_url));
    dyn_container(url, move |url| {
        let Some(url) = url else {
            return Empty::new().into_any();
        };
        tip(
            icon_button(
                || SidebarIcon::External,
                || theme::FG_MUTED,
                move || open_in_browser(&url),
            ),
            "Open on GitHub".to_string(),
        )
        .debug_name("github button")
        .into_any()
    })
    .style(move |s| control(s).apply_if(url().is_none(), |s| s.hide()))
}

/// Submit review for a pull request, Start work for an issue. A branch of
/// the reader's own has neither, and neither does a pull request that is
/// no longer open.
fn primary_action(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let is_issue = move || state.phase.get() == Phase::Issue;
    dyn_container(is_issue, move |is_issue| {
        if is_issue {
            crate::review::issue::start_work_button(app, state, backend.clone()).into_any()
        } else {
            crate::review::submit_review::control(state, backend.clone()).into_any()
        }
    })
    .style(move |s| {
        let present = is_issue() || crate::review::submit_review::is_available(state);
        control(s).apply_if(!present, |s| s.hide())
    })
}

/// The pane to the whole window, and the drawer back.
fn expand_button(app: AppState) -> impl IntoView {
    let open = move || app.sidebar_open.get();
    tip_with(
        icon_button(
            move || {
                if open() {
                    SidebarIcon::Expand
                } else {
                    SidebarIcon::Contract
                }
            },
            || theme::FG_MUTED,
            move || app.sidebar_open.update(|open| *open = !*open),
        ),
        move || {
            if open() {
                "Hide the sidebar".to_string()
            } else {
                "Show the sidebar".to_string()
            }
        },
    )
    .debug_name("expand button")
    .style(control)
}

/// A control in the bar. The bar's sides let the pointer through so a
/// press on them drags the window; a control takes it back.
fn control(s: floem::style::Style) -> floem::style::Style {
    s.flex_shrink(0.0_f32).pointer_events_auto()
}

/// Shown while a checkout is being prepared.
pub fn preparing_card(
    message: impl Fn() -> String + 'static,
    percent: impl Fn() -> Option<u8> + 'static,
) -> impl IntoView {
    Container::new(
        Stack::vertical((
            Container::new(sidebar_glyph(SidebarIcon::Refresh, theme::ACCENT)).style(|s| {
                s.size(44.0, 44.0)
                    .border_radius(22.0)
                    .background(theme::ACCENT_SUBTLE)
                    .items_center()
                    .justify_center()
                    .margin_bottom(14.0)
            }),
            Label::derived(|| "Preparing the checkout".to_string()).style(|s| {
                s.font_size(15.0)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG)
                    .margin_bottom(6.0)
            }),
            Label::derived(message).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::FG_MUTED)
                    .margin_bottom(14.0)
            }),
            progress_bar(percent),
        ))
        .style(|s| s.items_center().width(340.0)),
    )
    .style(|s| s.size_full().items_center().justify_center())
}

fn progress_bar(percent: impl Fn() -> Option<u8> + 'static) -> impl IntoView {
    Container::new(Empty::new().style(move |s| {
        // Without a percentage the bar sits at a third, which still reads as
        // motion rather than as a stalled zero.
        let fraction = percent().map(|p| p as f64 / 100.0).unwrap_or(0.33);
        s.height_full()
            .width_pct(fraction * 100.0)
            .background(theme::ACCENT)
            .border_radius(3.0)
    }))
    .style(|s| {
        s.width_full()
            .height(5.0)
            .background(theme::BADGE_BG)
            .border_radius(3.0)
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::kurbo::Rect;
    use perch_github::{CheckState, Item, ItemSubject};

    /// The first view whose debug name has `name` in it, depth first.
    pub(crate) fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    pub(crate) fn rect(harness: &HeadlessHarness, name: &str) -> Rect {
        named(harness.root_id(), name)
            .unwrap_or_else(|| panic!("no view named {name}"))
            .get_visual_rect_no_clip()
    }

    fn checkout() -> perch_github::ItemCheckout {
        perch_github::ItemCheckout {
            branch: "perch/pr-812".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: "/tmp/slatedb__slatedb__perch-pr-812".into(),
            owned: true,
            on_disk: true,
            churn: Some((504, 135)),
            last_seen_head: None,
        }
    }

    /// The whole window, 1400 by 900 with the drawer open, on an open
    /// pull request with a checkout and passing checks.
    pub(crate) fn pull_request_harness() -> (HeadlessHarness, AppState, ReviewState) {
        pull_request_harness_with(pull_request(812))
    }

    pub(crate) fn pull_request_harness_with(
        pull: perch_github::PullRequest,
    ) -> (HeadlessHarness, AppState, ReviewState) {
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "pr".into();
        item.subject = Some(ItemSubject::PullRequest(pull.clone()));
        item.checkout = Some(checkout());
        state.item.set(Some(Arc::new(item)));
        state.pull_request.set(Some(Arc::new(pull)));
        state.checks.set(Some(Arc::new(perch_github::Checks {
            state: CheckState::Passing,
            url: None,
            runs: Vec::new(),
        })));
        state.ready.set(true);
        state.phase.set(Phase::Ready);
        // The keys are driven through the window; no text box takes them.
        state.composer_autofocus.set(false);
        state.submit_review_autofocus.set(false);
        let root = TestRoot::new();
        let view = crate::ui::app_view(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
    }

    /// The same window on an open issue with nothing on it: no labels, no
    /// assignees, nothing started from it.
    pub(crate) fn issue_harness() -> (HeadlessHarness, AppState, ReviewState) {
        let app = AppState::new();
        let state = ReviewState::new();
        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let issue = perch_github::Issue {
            number: 400,
            title: "Trace reads".into(),
            body: String::new(),
            state: "open".into(),
            state_reason: None,
            author_login: "u".into(),
            author_avatar_url: String::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            comments: 0,
            created_at: None,
            updated_at: now,
            repo_full_name: "slatedb/slatedb".into(),
        };
        let mut item = Item::new("slatedb/slatedb", now);
        item.id = "issue".into();
        item.subject = Some(ItemSubject::Issue(issue.clone()));
        state.item.set(Some(Arc::new(item)));
        state.issue.set(Some(Arc::new(issue)));
        state.ready.set(true);
        state.phase.set(Phase::Issue);
        state.composer_autofocus.set(false);
        let root = TestRoot::new();
        let view = crate::ui::app_view(app, state, Rc::new(Backend::detached()));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
    }

    pub(crate) fn within(inner: Rect, outer: Rect) -> bool {
        inner.x0 >= outer.x0 && inner.x1 <= outer.x1 && inner.y0 >= outer.y0 && inner.y1 <= outer.y1
    }

    /// The drawer control moves from the drawer's strip to the item bar
    /// when the drawer shuts. The bar has a border along its bottom and
    /// the strip has none, and the control must not move up a pixel with
    /// the room the border takes.
    #[test]
    fn the_drawer_control_keeps_its_line_when_the_drawer_shuts() {
        let (mut harness, app, _) = pull_request_harness();
        let in_strip = rect(&harness, "drawer toggle");

        app.sidebar_open.set(false);
        harness.rebuild();
        harness.paint();
        let in_bar = rect(&harness, "drawer toggle");
        assert_eq!(
            (in_bar.y0, in_bar.y1),
            (in_strip.y0, in_strip.y1),
            "the control at {in_bar:?} is not on the strip's line {in_strip:?}"
        );
    }

    #[test]
    fn the_bar_is_the_title_the_switch_and_four_controls_in_a_row() {
        let (harness, ..) = pull_request_harness();
        let bar = rect(&harness, "item bar");
        assert_eq!(bar.height(), theme::REVIEW_HEADER_HEIGHT);
        assert_eq!(
            bar.x0,
            theme::SIDEBAR_WIDTH,
            "the bar starts at the drawer's edge"
        );
        assert_eq!(bar.y0, 0.0, "and at the window's top: it is the title bar");

        // Left to right: the title, the switch at the centre, then GitHub,
        // Review, Submit review and the expand control against the right
        // edge.
        let title = rect(&harness, "item title");
        let switch = rect(&harness, "item switch");
        let github = rect(&harness, "github button");
        let review = rect(&harness, "review-launch-control");
        let submit = rect(&harness, "submit-review-control");
        let expand = rect(&harness, "expand button");
        for (name, r) in [
            ("title", title),
            ("switch", switch),
            ("github", github),
            ("review", review),
            ("submit", submit),
            ("expand", expand),
        ] {
            assert!(within(r, bar), "{name} at {r:?} is not in the bar {bar:?}");
            assert!(r.width() > 0.0, "{name} has no width");
        }
        assert!(title.x1 <= switch.x0);
        assert!(switch.x1 <= github.x0);
        assert!(github.x1 <= review.x0);
        assert!(review.x1 <= submit.x0);
        assert!(submit.x1 <= expand.x0);
        assert_eq!(
            review.height(),
            submit.height(),
            "Review and Submit review are buttons of one height"
        );
        assert!(
            (switch.center().x - bar.center().x).abs() <= 1.0,
            "the switch at {switch:?} is off the bar's centre {}",
            bar.center().x
        );
        assert_eq!(
            expand.x1,
            bar.x1 - theme::REVIEW_HEADER_PADDING,
            "expand sits against the bar's padding"
        );
        // With the drawer open there is no lane before the title: the
        // state glyph and its gap are all that is.
        assert_eq!(title.x0, bar.x0 + theme::REVIEW_HEADER_PADDING + 16.0 + 8.0);
    }

    #[test]
    fn a_long_title_gives_way_and_the_switch_stays_centred() {
        let mut pull = pull_request(812);
        pull.title = "Add tracing spans to every read path, ".repeat(8);
        let (harness, ..) = pull_request_harness_with(pull);
        let bar = rect(&harness, "item bar");
        let title = rect(&harness, "item title");
        let switch = rect(&harness, "item switch");
        assert!(
            title.width() < bar.width() / 2.0,
            "the title at {title:?} did not give way"
        );
        assert!(title.x1 <= switch.x0);
        assert!((switch.center().x - bar.center().x).abs() <= 1.0);
        assert_eq!(
            rect(&harness, "expand button").x1,
            bar.x1 - theme::REVIEW_HEADER_PADDING
        );
    }

    /// Whether `id` is `ancestor` or under it.
    fn descends_from(mut id: floem::ViewId, ancestor: floem::ViewId) -> bool {
        loop {
            if id == ancestor {
                return true;
            }
            match id.parent() {
                Some(parent) => id = parent,
                None => return false,
            }
        }
    }

    /// The bar's popovers hang from it over the item, and nothing in the
    /// item paints over them.
    fn popover_covers_the_item(harness: &HeadlessHarness, popover: floem::ViewId) {
        let bounds = popover.get_visual_rect_no_clip();
        let x = bounds.x0 + 2.0;
        let title_y = theme::REVIEW_HEADER_HEIGHT + theme::CONVERSATION_PADDING + 10.0;
        let summary_y = title_y + theme::TEXT_PAGE_TITLE as f64 + theme::CONVERSATION_TITLE_GAP;
        assert!(
            bounds.y0 < title_y && bounds.y1 > summary_y,
            "popover at {bounds:?} does not reach the summary"
        );
        for (layer, y) in [("title", title_y), ("summary", summary_y)] {
            let target = harness
                .element_id_at(x, y)
                .unwrap_or_else(|| panic!("nothing under the {layer}"))
                .owning_id();
            assert!(
                descends_from(target, popover),
                "the {layer} painted over the popover"
            );
        }
    }

    #[test]
    fn submit_review_opens_from_the_bar_over_the_item() {
        let (mut harness, _, state) = pull_request_harness();
        let control = rect(&harness, "submit-review-control").center();
        harness.click(control.x, control.y);
        harness.rebuild();
        harness.paint();
        assert!(state.submit_review_open.get(), "the click did not open it");
        let popover = named(harness.root_id(), "submit-review-popover").unwrap();
        popover_covers_the_item(&harness, popover);
    }

    #[test]
    fn the_review_launcher_opens_from_the_bar_over_the_item() {
        let (mut harness, _, state) = pull_request_harness();
        let control = rect(&harness, "review-launch-control").center();
        harness.click(control.x, control.y);
        harness.rebuild();
        harness.paint();
        assert!(state.review_launcher_open.get(), "the click did not open it");
        let popover = named(harness.root_id(), "review-launch-popover").unwrap();
        popover_covers_the_item(&harness, popover);

        let close = rect(&harness, "review-launch-close").center();
        harness.click(close.x, close.y);
        assert!(!state.review_launcher_open.get());
    }

    #[test]
    fn nothing_sits_between_the_bar_and_the_item() {
        let (harness, ..) = pull_request_harness();
        let bar = rect(&harness, "item bar");
        let title = rect(&harness, "conversation-title");
        assert_eq!(
            title.y0,
            bar.y1 + theme::CONVERSATION_PADDING,
            "the page starts under the bar with the column's own padding"
        );
    }

    #[test]
    fn the_bar_drags_beside_the_title_and_its_controls_keep_their_clicks() {
        let (mut harness, app, _) = pull_request_harness();
        let bar = named(harness.root_id(), "item bar").unwrap();
        let title = rect(&harness, "item title");
        let switch = rect(&harness, "item switch");

        // The room between the title and the switch is the bar's own, so a
        // press there is a drag and not a click on anything.
        let x = (title.x1 + switch.x0) / 2.0;
        let y = rect(&harness, "item bar").center().y;
        let hit = harness.element_id_at(x, y).expect("nothing under the bar");
        assert_eq!(hit.owning_id(), bar, "the bar's side took the press");

        // The expand control shuts the drawer: the bar then runs from the
        // window's edge, with the lights lane and the drawer control
        // before the title, and the control brings the drawer back.
        let expand = rect(&harness, "expand button").center();
        harness.click(expand.x, expand.y);
        harness.rebuild();
        harness.paint();
        assert!(!app.sidebar_open.get());
        let bar = rect(&harness, "item bar");
        assert_eq!(bar.x0, 0.0);
        let toggle = rect(&harness, "drawer toggle");
        assert!(
            within(toggle, bar),
            "the drawer toggle at {toggle:?} is not in the bar"
        );
        assert_eq!(toggle.x0, theme::ACTIVITY_RAIL_WIDTH, "after the lights");
        assert!(toggle.x1 <= rect(&harness, "item title").x0);
        // Beside the lights is the bar's own surface too.
        let hit = harness
            .element_id_at(theme::ACTIVITY_RAIL_WIDTH / 2.0, bar.center().y)
            .expect("nothing under the lane");
        assert_eq!(
            hit.owning_id(),
            named(harness.root_id(), "item bar").unwrap(),
            "the lane took the press"
        );
        harness.click(toggle.center().x, toggle.center().y);
        harness.rebuild();
        harness.paint();
        assert!(app.sidebar_open.get());
        assert_eq!(rect(&harness, "item bar").x0, theme::SIDEBAR_WIDTH);
    }

    #[test]
    fn an_issue_has_start_work_where_a_pull_request_has_submit_review() {
        let (harness, ..) = issue_harness();
        let bar = rect(&harness, "item bar");
        assert!(named(harness.root_id(), "submit-review-control").is_none());
        // An issue has no files for an agent to review.
        assert!(named(harness.root_id(), "review-launch-control").is_none());
        let github = rect(&harness, "github button");
        let start = rect(&harness, "start work button");
        let expand = rect(&harness, "expand button");
        for (name, r) in [
            ("github", github),
            ("start work", start),
            ("expand", expand),
        ] {
            assert!(within(r, bar), "{name} at {r:?} is not in the bar {bar:?}");
            assert!(r.width() > 0.0, "{name} has no width");
        }
        assert!(github.x1 <= start.x0 && start.x1 <= expand.x0);
    }

    pub(crate) fn pull_request(number: u64) -> perch_github::PullRequest {
        perch_github::PullRequest {
            number,
            title: format!("pull {number}"),
            html_url: format!("https://github.com/slatedb/slatedb/pull/{number}"),
            author_login: "u".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "feature".into(),
            base_sha: "base".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            repo_full_name: "slatedb/slatedb".into(),
            head_repo_full_name: Some("slatedb/slatedb".into()),
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: Vec::new(),
            milestone: None,
            labels: Vec::new(),
            assignees: Vec::new(),
        }
    }
}
