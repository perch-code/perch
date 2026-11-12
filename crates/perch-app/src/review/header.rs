//! The bar above the item view.
//!
//! The bar is the window's title bar. It holds what is open, the switch
//! between Conversation and Code, and the things a reader reaches for from
//! anywhere in an item, as Codex's bar has them: a menu of what can be
//! done with the item, GitHub among it; its details, which are its
//! branches, its worktree, what GitHub says about it and every item's
//! agents; Start work until something has been started; Merge on a pull
//! request the reader may merge; Review on anything with files; and
//! closing the item.
//! The item's state and number, its checks and who opened it are
//! `review::summary`, one line under Conversation's title.

use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::text::TextOverflowChanged;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::{self, SidebarIcon};
use crate::review::cards::{tip, tip_with};
use crate::review::model::{Phase, ReviewState};
use crate::review::summary::subject_state;
use crate::state::AppState;
use crate::theme;
use crate::ui::items::github_url;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::titlebar::{part_of_bar, title_bar};
use crate::ui::widgets::{copy_to_clipboard, icon_button, open_in_browser, sidebar_glyph};

/// The bar, the same for every kind of item: a pull request, an issue, or
/// a worktree of the reader's own.
pub fn header(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // The bar is the window's title bar, so a press on anything in it that
    // is not a control drags the window. Its three parts let the pointer
    // through, and each control in them takes it back.
    title_bar(
        Stack::horizontal((
            leading(app, state, backend.clone()),
            crate::review::switch::switch(state),
            trailing(app, state, backend),
        ))
        .debug_name("item bar"),
    )
    // Floem bounds z-index to sibling stacking contexts. Review and the
    // Agents dropdown open from this bar, so the bar sits above the item.
    .style(|s| bar_style(s).z_index(2))
}

pub(crate) fn bar_style(s: floem::style::Style) -> floem::style::Style {
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
        .background(theme::canvas())
        .border_bottom(1.0)
        .border_color(theme::border_muted())
}

/// Either side of the switch. Both let the pointer through, so a press
/// between the controls drags the window. While the switch shows, the two
/// take the bar's room in equal shares, so it sits at the bar's centre
/// whatever is on either side of it; the title on the left gives way
/// first, since the controls on the right hold their size. Without it,
/// the title takes the room the controls leave.
fn side(s: floem::style::Style) -> floem::style::Style {
    s.items_center().pointer_events_none()
}

/// What is open: the lights and the drawer control while the drawer is
/// shut, Back and Forward, then the item's state as a glyph, and its
/// title.
fn leading(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::horizontal((
        crate::ui::sidebar::pane_lights_lane(app),
        crate::ui::history::buttons(app, state, backend),
        state_glyph(state),
        item_title(state),
    ))
    .style(|s| {
        side(s)
            .flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .gap(8.0)
    })
}

/// The title, cut short with an ellipsis when the bar has no room for all
/// of it, and then the whole of it is a tip on hover. It takes the pointer
/// only while it is cut short, and a press on it still drags the window.
fn item_title(state: ReviewState) -> impl IntoView {
    let cut_short = RwSignal::new(false);
    let label = Label::derived(move || title(state))
        .debug_name("item title")
        .on_event(
            TextOverflowChanged::listener(),
            move |_, change: &TextOverflowChanged| {
                cut_short.set(change.is_overflowing);
                EventPropagation::Continue
            },
        )
        .style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
                .apply_if(cut_short.get(), |s| s.pointer_events_auto())
        });
    tip_with(part_of_bar(label), move || title(state)).style(|s| s.min_width(0.0))
}

/// The pull request's or the issue's title. A worktree of the reader's own
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
            None => sidebar_glyph(SidebarIcon::Branch, theme::fg_muted)
                .style(|s| s.size(16.0, 16.0))
                .into_any(),
        },
    )
    .style(|s| s.flex_shrink(0.0_f32))
}

/// What is reached for from anywhere in an item: its menu, its details
/// and every item's agents, Start work, Merge, Review, and closing it.
/// Bug reports and plan usage are the app's, and sit in the drawer's
/// footer.
fn trailing(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Stack::horizontal((
        more_button(app, state),
        crate::ui::agents::dropdown::button(app, state, backend.clone()),
        crate::review::start_work::button(app, state),
        crate::review::merge_button::control(app, state, backend.clone()),
        review_control(app, state, backend.clone()),
        close_button(app, state, backend),
    ))
    .style(move |s| {
        side(s)
            .flex_shrink(0.0_f32)
            .gap(8.0)
            .justify_end()
            .apply_if(state.has_files(), |s| s.flex_grow(1.0_f32).flex_basis(0.0))
    })
}

/// The three dots: a menu of what can be done with the item. Absent
/// while there is no item to do it with.
fn more_button(app: AppState, state: ReviewState) -> impl IntoView {
    icon_button(|| SidebarIcon::Ellipsis, theme::fg_muted, || {})
        .into_view()
        .dropdown(move || more_menu(app, state))
        .debug_name("more button")
        .style(move |s| {
            control(s)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
                .apply_if(state.item.with(Option::is_none), |s| s.hide())
        })
}

/// The bar's menu: the pull request's or the issue's page on github.com
/// and its link, the branch's name, and Start work once it has left the
/// bar. A worktree of the reader's own is nowhere on GitHub, and has the
/// last two.
fn more_menu(app: AppState, state: ReviewState) -> Menu {
    let Some(item) = state.item.get_untracked() else {
        return Menu::new();
    };
    let mut menu = Menu::new();
    if let Some(url) = github_url(&item) {
        let link = url.clone();
        menu = menu
            .item("Open on GitHub", move |i| {
                i.icon(SidebarIcon::External)
                    .action(move || open_in_browser(&url))
            })
            .item("Copy Link", move |i| {
                i.icon(SidebarIcon::Link)
                    .action(move || copy_to_clipboard(link.clone()))
            });
    }
    let branch = item
        .pull_request()
        .map(|pull| pull.head_ref.clone())
        .or_else(|| item.checkout.as_ref().map(|c| c.branch.clone()))
        .filter(|branch| !branch.is_empty());
    if let Some(branch) = branch {
        menu = menu.item("Copy Branch Name", move |i| {
            i.icon(SidebarIcon::Copy)
                .action(move || copy_to_clipboard(branch.clone()))
        });
    }
    let in_bar = floem::reactive::Effect::untrack(|| crate::review::start_work::in_bar(app, &item));
    if crate::review::start_work::can_start_work(&item) && !in_bar {
        use crate::review::start_work::{ask, Pending};
        let id = item.id.clone();
        menu = menu.separator().item("Start Work", move |i| {
            i.icon(SidebarIcon::Branch)
                .action(move || ask(app, Pending::Item(id.clone())))
        });
    }
    menu
}

/// Review for anything with files: a pull request, or a worktree of the
/// reader's own. An issue has none until work starts on it.
fn review_control(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let is_issue = move || state.phase.get() == Phase::Issue;
    dyn_container(is_issue, move |is_issue| {
        if is_issue {
            Empty::new().into_any()
        } else {
            crate::review::review_menu::control(app, state, backend.clone()).into_any()
        }
    })
    .style(move |s| control(s).apply_if(is_issue(), |s| s.hide()))
}

/// Close the item, as Cmd-[ does, and leave the main area empty.
fn close_button(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    tip(
        icon_button(
            || SidebarIcon::Close,
            theme::fg_muted,
            move || {
                let navigation_id = backend.close_review();
                app.begin_close(state, navigation_id);
            },
        ),
        format!(
            "Close ({})",
            crate::ui::menu::Shortcut::label_of("super+BracketLeft")
        ),
    )
    .debug_name("close button")
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
            Container::new(sidebar_glyph(SidebarIcon::Refresh, theme::accent)).style(|s| {
                s.size(44.0, 44.0)
                    .border_radius(22.0)
                    .background(theme::accent_subtle())
                    .items_center()
                    .justify_center()
                    .margin_bottom(14.0)
            }),
            Label::derived(|| "Preparing the checkout".to_string()).style(|s| {
                s.font_size(theme::TEXT_SECTION_TITLE)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
                    .margin_bottom(6.0)
            }),
            Label::derived(message).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .color(theme::fg_muted())
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
            .background(theme::accent())
            .border_radius(3.0)
    }))
    .style(|s| {
        s.width_full()
            .height(5.0)
            .background(theme::badge_bg())
            .border_radius(3.0)
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use floem::reactive::SignalUpdate;

    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::kurbo::Rect;
    use perch_github::{CheckState, Item, ItemSubject};

    use crate::review::model::ItemTab;

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
            divergence: None,
            source: None,
        }
    }

    /// The whole window, 1400 by 900 with the drawer open, on an open
    /// pull request with a checkout and passing checks.
    /// A pull request's bar and summary, all of it dark once the system is.
    #[test]
    fn goes_dark_with_the_system() {
        let mut harness = pull_request_harness().0;
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(
            left.is_empty(),
            "light colours left in dark:\n{}",
            left.join("\n")
        );
    }

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
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
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

    /// Open the bar's details, as a press on their button does.
    pub(crate) fn open_details(harness: &mut HeadlessHarness, state: ReviewState) {
        state.agents_dropdown_open.set(true);
        harness.rebuild();
        harness.paint();
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
    fn the_bar_is_the_title_the_switch_and_five_controls_in_a_row() {
        let (mut harness, app, _) = pull_request_harness();
        let bar = rect(&harness, "item bar");
        assert_eq!(bar.height(), theme::REVIEW_HEADER_HEIGHT);
        assert_eq!(
            bar.x0,
            theme::SIDEBAR_WIDTH,
            "the bar starts at the drawer's edge"
        );
        assert_eq!(bar.y0, 0.0, "and at the window's top: it is the title bar");

        // Left to right: Back and Forward, the title, the switch, then
        // the menu, the details, Start work, Review, and close against
        // the right edge.
        let title = rect(&harness, "item title");
        let switch = rect(&harness, "item switch");
        let github = rect(&harness, "more button");
        let agents = rect(&harness, "agents-button");
        let start = rect(&harness, "start work button");
        let review = rect(&harness, "review-control");
        let close = rect(&harness, "close button");
        for (name, r) in [
            ("title", title),
            ("switch", switch),
            ("more", github),
            ("details", agents),
            ("start work", start),
            ("review", review),
            ("close", close),
        ] {
            assert!(within(r, bar), "{name} at {r:?} is not in the bar {bar:?}");
            assert!(r.width() > 0.0, "{name} has no width");
        }
        assert!(title.x1 <= switch.x0);
        assert!(switch.x1 <= github.x0);
        assert!(github.x1 <= agents.x0);
        assert!(agents.x1 <= start.x0);
        assert!(start.x1 <= review.x0);
        assert!(review.x1 <= close.x0);
        assert_eq!(
            close.x1,
            bar.x1 - theme::REVIEW_HEADER_PADDING,
            "close sits against the bar's padding"
        );
        // With the drawer open there is no lane before the title: Back
        // and Forward, the state glyph and their gaps are all that is.
        let arrows = rect(&harness, "history arrows");
        assert_eq!(arrows.x0, bar.x0 + theme::REVIEW_HEADER_PADDING);
        assert_eq!(arrows.width(), 2.0 * 26.0);
        assert_eq!(title.x0, arrows.x1 + 8.0 + 16.0 + 8.0);
        // The switch is at the bar's centre, with the drawer open and
        // with it shut.
        for open in [true, false] {
            app.sidebar_open.set(open);
            harness.rebuild();
            harness.paint();
            let (bar, switch) = (rect(&harness, "item bar"), rect(&harness, "item switch"));
            assert!(
                (switch.center().x - bar.center().x).abs() <= 1.0,
                "the switch at {switch:?} is not at the bar's centre {bar:?}"
            );
        }
    }

    #[test]
    fn a_long_title_gives_way_to_the_controls() {
        let mut pull = pull_request(812);
        pull.title = "Add tracing spans to every read path, ".repeat(8);
        let (mut harness, app, _) = pull_request_harness_with(pull);
        app.sidebar_open.set(false);
        harness.rebuild();
        harness.paint();
        let bar = rect(&harness, "item bar");
        let title = rect(&harness, "item title");
        let switch = rect(&harness, "item switch");
        assert!(
            title.x1 <= switch.x0,
            "the title at {title:?} did not give way to {switch:?}"
        );
        assert!(
            (switch.center().x - bar.center().x).abs() <= 1.0,
            "the title pushed the switch off the centre"
        );
        assert_eq!(
            rect(&harness, "close button").x1,
            bar.x1 - theme::REVIEW_HEADER_PADDING
        );
    }

    /// A title that is cut short takes the pointer, so hovering it shows
    /// the whole of it. One that fits is the bar's own surface.
    #[test]
    fn only_a_title_cut_short_takes_the_pointer() {
        let (harness, _, _) = pull_request_harness();
        let title = rect(&harness, "item title").center();
        let hit = harness.element_id_at(title.x, title.y).unwrap();
        assert_eq!(
            hit.owning_id(),
            named(harness.root_id(), "item bar").unwrap(),
            "a title that fits took the pointer"
        );

        let mut pull = pull_request(812);
        pull.title = "Add tracing spans to every read path, ".repeat(8);
        let (mut harness, _, _) = pull_request_harness_with(pull);
        harness.rebuild();
        harness.paint();
        let title = rect(&harness, "item title").center();
        let hit = harness.element_id_at(title.x, title.y).unwrap();
        assert_eq!(
            hit.owning_id(),
            named(harness.root_id(), "item title").unwrap(),
            "a title cut short let the pointer through"
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
        let first_y = theme::REVIEW_HEADER_HEIGHT + theme::CONVERSATION_PADDING + 10.0;
        let second_y = first_y + theme::SUMMARY_ROW;
        assert!(
            bounds.y0 < first_y && bounds.y1 > second_y,
            "popover at {bounds:?} does not reach the summary"
        );
        for (layer, y) in [("summary's first row", first_y), ("its second", second_y)] {
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

    /// On a pull request, Review is the agents folded to a line over the
    /// reader's own review; Settings unfolds them and folds the review.
    #[test]
    fn review_opens_from_the_bar_over_the_item() {
        let (mut harness, _, state) = pull_request_harness();
        let control = rect(&harness, "review-control").center();
        harness.click(control.x, control.y);
        harness.rebuild();
        harness.paint();
        assert!(state.review_open.get(), "the click did not open it");
        let popover = named(harness.root_id(), "review-popover").unwrap();
        popover_covers_the_item(&harness, popover);
        let agents = rect(&harness, "review-agents");
        let submit = rect(&harness, "submit-review");
        assert!(agents.y1 <= submit.y0, "the agents are not over the review");
        assert!(rect(&harness, "review-agents-summary").width() > 0.0);
        assert_eq!(rect(&harness, "review-instructions").height(), 0.0);

        let settings = rect(&harness, "review-agents-toggle").center();
        harness.click(settings.x, settings.y);
        harness.rebuild();
        harness.paint();
        assert!(rect(&harness, "review-instructions").height() > 0.0);
        assert!(named(harness.root_id(), "submit-review-editor").is_none());
        let unfold = rect(&harness, "submit-review-unfold").center();
        harness.click(unfold.x, unfold.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(rect(&harness, "review-instructions").height(), 0.0);
        assert!(named(harness.root_id(), "submit-review-editor").is_some());

        let close = rect(&harness, "review-close").center();
        harness.click(close.x, close.y);
        assert!(!state.review_open.get());
    }

    /// Conversation starts under the bar, with no tabs between; Code's
    /// tabs start under the bar.
    #[test]
    fn conversation_is_under_the_bar_and_codes_tabs_are_under_it_too() {
        let (mut harness, _, state) = pull_request_harness();
        let bar = rect(&harness, "item bar");
        let column = rect(&harness, "conversation column");
        assert!(
            named(harness.root_id(), "tab strip").is_none(),
            "Conversation has no tabs"
        );
        assert_eq!(
            column.y0,
            bar.y1 + theme::CONVERSATION_PADDING,
            "the page starts under the bar with the column's own padding"
        );

        let code = rect(&harness, "Code pill").center();
        harness.click(code.x, code.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.tab.get(), ItemTab::Code);
        let strip = rect(&harness, "tab strip");
        assert_eq!(strip.y0, bar.y1, "the tabs start under the bar");
        assert_eq!(strip.height(), theme::TAB_HEIGHT);
        assert!(named(harness.root_id(), "conversation column").is_none());

        let conversation = rect(&harness, "Conversation pill").center();
        harness.click(conversation.x, conversation.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert!(named(harness.root_id(), "tab strip").is_none());
    }

    /// An issue with no checkout has no switch, and its title takes the
    /// room the controls leave. Start work gives it a checkout, and the
    /// switch comes.
    #[test]
    fn an_issue_has_no_switch_until_it_has_a_checkout() {
        let (mut harness, _, state) = issue_harness();
        let bar = rect(&harness, "item bar");
        assert_eq!(rect(&harness, "item switch").width(), 0.0);
        assert!(named(harness.root_id(), "tab strip").is_none());
        let title = rect(&harness, "item title");
        let github = rect(&harness, "more button");
        assert!(title.x1 <= github.x0);

        let mut item = (*state.item.get().unwrap()).clone();
        item.checkout = Some(checkout());
        state.item.set(Some(Arc::new(item)));
        harness.rebuild();
        harness.paint();
        let switch = rect(&harness, "item switch");
        assert!(switch.width() > 0.0, "no switch with a checkout");
        assert!((switch.center().x - bar.center().x).abs() <= 1.0);
    }

    /// An issue reading the default branch has Code: the switch comes
    /// with the home, and Code's rail names the branch, its commit and a
    /// refresh in place of a scope.
    #[test]
    fn an_issue_with_its_home_has_code_on_the_default_branch() {
        let (mut harness, app, state) = issue_harness();
        assert_eq!(rect(&harness, "item switch").width(), 0.0);

        state.home.set(Some(Arc::new(crate::backend::HomeCode {
            path: "/tmp/home".into(),
            branch: "main".into(),
            head_sha: "0123456789abcdef0123456789abcdef01234567".into(),
            fetched_at: Some(app.now.get_untracked()),
        })));
        state.tree.set(Arc::new(vec![perch_git::TreeEntry::new(
            "README.md",
            false,
        )]));
        harness.rebuild();
        harness.paint();
        assert!(rect(&harness, "item switch").width() > 0.0, "no switch");

        state.show_tab(ItemTab::Code);
        app.tree_open.set(true);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "home-row").is_some());
        assert!(named(harness.root_id(), "Label: \"main\"").is_some());
        assert!(named(harness.root_id(), "Label: \"0123456\"").is_some());
        assert!(named(harness.root_id(), "Label: \"fetched just now\"").is_some());
    }

    #[test]
    fn the_bar_drags_beside_the_title_and_its_controls_keep_their_clicks() {
        let (mut harness, app, _) = pull_request_harness();
        let bar = named(harness.root_id(), "item bar").unwrap();
        let title = rect(&harness, "item title");

        // The room between the title and the switch is the bar's own, so
        // a press there is a drag and not a click on anything.
        let x = (title.x1 + rect(&harness, "item switch").x0) / 2.0;
        let y = rect(&harness, "item bar").center().y;
        let hit = harness.element_id_at(x, y).expect("nothing under the bar");
        assert_eq!(hit.owning_id(), bar, "the bar's side took the press");

        // With the drawer shut the bar runs from the window's edge, with
        // the lights lane and the drawer control before the title, and the
        // control brings the drawer back.
        app.sidebar_open.set(false);
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
    fn an_issue_has_start_work_where_a_pull_request_has_review() {
        let (harness, ..) = issue_harness();
        let bar = rect(&harness, "item bar");
        // An issue has no files to review.
        assert!(named(harness.root_id(), "review-control").is_none());
        let github = rect(&harness, "more button");
        let start = rect(&harness, "start work button");
        let close = rect(&harness, "close button");
        for (name, r) in [("more", github), ("start work", start), ("close", close)] {
            assert!(within(r, bar), "{name} at {r:?} is not in the bar {bar:?}");
            assert!(r.width() > 0.0, "{name} has no width");
        }
        assert!(github.x1 <= start.x0 && start.x1 <= close.x0);
    }

    /// The menu has GitHub, the link and the branch's name. Start work is
    /// a button in the bar until something has been started from the
    /// item, and a row in the menu after; a worktree of the reader's own
    /// has it in the menu from the start.
    #[test]
    fn the_menu_has_github_and_start_work_once_it_has_left_the_bar() {
        let (mut harness, app, state) = pull_request_harness();
        assert_eq!(
            more_menu(app, state).outline(),
            ["Open on GitHub", "Copy Link", "Copy Branch Name"]
        );
        assert!(rect(&harness, "start work button").width() > 0.0);

        let now = "2026-09-20T12:00:00Z".parse().unwrap();
        let mut child = Item::new("slatedb/slatedb", now);
        child.id = "child".into();
        child.from_item_id = Some("pr".into());
        app.items.set(vec![child]);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "start work button").is_none());
        let menu = more_menu(app, state);
        assert_eq!(
            menu.outline(),
            [
                "Open on GitHub",
                "Copy Link",
                "Copy Branch Name",
                "—",
                "Start Work"
            ]
        );
        menu.choose("Start Work");
        assert!(app.start_work_confirm.with(Option::is_some));

        crate::ui::widgets::opened();
        menu.choose("Open on GitHub");
        assert_eq!(
            crate::ui::widgets::opened(),
            ["https://github.com/slatedb/slatedb/pull/812"]
        );

        // A press on the dots opens it.
        let more = rect(&harness, "more button").center();
        harness.pointer_down(more.x, more.y);
        harness.rebuild();
        harness.paint();
        assert!(crate::ui::menu::is_open());
        assert!(named(harness.root_id(), "menu-row:Open on GitHub").is_some());
        crate::ui::menu::close();

        // A worktree of the reader's own is nowhere on GitHub.
        let mut branch = Item::new("slatedb/slatedb", now);
        branch.id = "branch".into();
        branch.checkout = Some(checkout());
        state.pull_request.set(None);
        state.item.set(Some(Arc::new(branch)));
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "start work button").is_none());
        assert_eq!(
            more_menu(app, state).outline(),
            ["Copy Branch Name", "—", "Start Work"]
        );
    }

    /// The details open from the list in the bar: the item's rows over
    /// the agents' runs, and a press outside puts them away.
    #[test]
    fn the_details_open_from_the_bar_over_the_agents() {
        let (mut harness, _, state) = pull_request_harness();
        assert!(named(harness.root_id(), "item details").is_none());
        let button = rect(&harness, "agents-button").center();
        harness.click(button.x, button.y);
        harness.rebuild();
        harness.paint();
        assert!(state.agents_dropdown_open.get());
        let popover = rect(&harness, "agents-dropdown");
        let details = rect(&harness, "item details");
        assert!(details.height() > 0.0);
        assert!(
            within(details, popover),
            "the details at {details:?} are not in {popover:?}"
        );
        assert_eq!(
            details.y0,
            popover.y0 + 1.0,
            "the details are not on top, under the border"
        );
        let branch = rect(&harness, "summary row Branch");
        let worktree = rect(&harness, "worktree menu");
        assert!(within(branch, details));
        assert!(
            within(worktree, rect(&harness, "summary row Worktree")),
            "the worktree at {worktree:?} is not on its row"
        );
        assert!(
            worktree.x1 <= details.x1,
            "{worktree:?} runs past {details:?}"
        );

        let title = rect(&harness, "item title").center();
        harness.click(title.x, title.y);
        harness.rebuild();
        harness.paint();
        assert!(!state.agents_dropdown_open.get());
        assert!(named(harness.root_id(), "item details").is_none());
    }

    /// The X closes the item, as Cmd-[ does.
    #[test]
    fn the_close_button_closes_the_item() {
        let (mut harness, app, state) = pull_request_harness();
        app.open_item.set(Some("pr".into()));
        let close = rect(&harness, "close button").center();
        harness.click(close.x, close.y);
        harness.rebuild();
        harness.paint();
        assert_eq!(app.open_item.get(), None);
        assert!(!state.is_showing(), "the item is still showing");
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
