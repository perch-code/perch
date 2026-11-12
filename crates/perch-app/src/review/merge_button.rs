//! Merge in the bar: a split button left of Review, and the merge box in a
//! popover under it.
//!
//! The button is named for the merge method picked, and its chevron picks
//! another of those the repository allows. It is GitHub's green when the
//! pull request could go in now, and quiet when something stands in the
//! way; either way a click opens the popover, which is `merge`'s box: what
//! stands in the way and what can be done about it, the commit message,
//! the bypass an admin may consent to, and the confirmation. On a draft
//! the reader may mark ready it is Ready for review instead, which does
//! that. There is none on an issue, a branch, a pull request merged or
//! closed, offline, on an old review's snapshot, or for a reader who may
//! not merge. The bar is the same on Conversation and Code, so it is
//! there on both.

use std::cell::Cell;
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::merge::{BarMerge, MergeBox};
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::menu::MenuExt;
use crate::ui::widgets::{clickable, icon_action, sidebar_glyph};

/// The bar wide enough for Merge to be named for its method in full, as
/// `Squash and merge`, beside the rest of its controls without pushing
/// the switch off the bar's centre. Narrower, as at 1400 with the drawer
/// open, it is plain `Merge`; the chevron's menu still checks the method,
/// and the box names it as the merge is confirmed.
const FULL_LABEL_ROOM: f64 = 1080.0;

/// Which button the bar has, so it is built again only when that changes
/// and not as its label does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    None,
    ReadyForReview,
    Merge,
}

/// The button, and the popover under it while it is open.
pub fn control(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let merge = MergeBox::new(app, state);
    // Kept while the overview is read again, as the summary's chips are,
    // so the button does not go for the moment of each refresh.
    let bar = crate::review::summary::kept_while_reading(state, move || merge.bar(state));
    let kind = Memo::new(move |_| match bar.get() {
        None => Kind::None,
        Some(BarMerge::ReadyForReview { .. }) => Kind::ReadyForReview,
        Some(BarMerge::Merge { .. }) => Kind::Merge,
    });
    // The main part, which has the keyboard back when the popover shuts.
    let main_id = RwSignal::new(None::<floem::ViewId>);
    // Set when the popover opens on its own, a merge having failed after
    // it was shut: it says why, but leaves the keyboard where the reader
    // has it, likely mid-sentence in a composer.
    let quiet = RwSignal::new(false);

    // The box goes with the button: the pull request merged, closed, made
    // a draft, or the reader's leave gone.
    Effect::new(move |_| {
        if kind.get() != Kind::Merge && state.merge_open.get() {
            state.merge_open.set(false);
        }
    });
    // Another of the bar's popovers opening puts this one away. They shut
    // each other as they open, but know nothing of this one.
    Effect::new(move |_| {
        let other = state.review_open.get()
            || state.agents_dropdown_open.get()
            || state.usage_open.get()
            || state.feedback_open.get();
        if other && state.merge_open.get_untracked() {
            state.merge_open.set(false);
        }
    });
    // Shutting it puts a confirmation away, and the error a merge ended
    // in with it; a merge under way goes on, and says how it went when it
    // is done.
    Effect::new(move |was: Option<bool>| {
        let open = state.merge_open.get();
        if was == Some(true) && !open {
            quiet.set(false);
            merge.cancel();
            if !state.merge_pending.get_untracked() {
                state.merge_error.set(None);
            }
        }
        open
    });
    // A merge that fails says why, though the popover was shut meanwhile.
    Effect::new(move |was: Option<bool>| {
        let failed = state.merge_error.with(Option::is_some);
        if failed
            && was == Some(false)
            && kind.get_untracked() == Kind::Merge
            && !state.merge_open.get_untracked()
        {
            quiet.set(true);
            open(state);
        }
        failed
    });

    let button_backend = backend.clone();
    let button = dyn_container(
        move || kind.get(),
        move |kind| match kind {
            Kind::None => Empty::new().into_any(),
            Kind::ReadyForReview => ready_button(state, bar, button_backend.clone()).into_any(),
            Kind::Merge => split_button(app, merge, state, bar, main_id).into_any(),
        },
    );

    // On a Memo: the signal notifies on every write, and each rebuild
    // would be a fresh popover taking the keyboard again.
    let shown = Memo::new(move |_| state.merge_open.get());
    Stack::new((
        button,
        dyn_container(
            move || shown.get(),
            move |shown| {
                if shown {
                    let quiet = quiet.get_untracked();
                    popover(state, backend.clone(), merge, main_id, quiet).into_any()
                } else {
                    Empty::new().into_any()
                }
            },
        )
        .style(|s| {
            s.position(Position::Absolute)
                .inset_top(34.0)
                .inset_right(0.0)
                .z_index(100)
        }),
    ))
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if key.code == Code::Escape && key.modifiers.is_empty() && state.merge_open.get_untracked()
        {
            close(state);
            if let Some(id) = main_id.get_untracked() {
                id.request_focus();
            }
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name("merge-control")
    .style(move |s| {
        // A control in the bar, which lets the pointer through elsewhere
        // so a press beside the controls drags the window.
        s.position(Position::Relative)
            .flex_shrink(0.0_f32)
            .pointer_events_auto()
            .apply_if(kind.get() == Kind::None, |s| s.hide())
    })
}

/// Open the merge box over whatever else hangs from the bar, as the bar's
/// Merge does and the summary's Conflicts chip, whose resolver is in it.
pub(crate) fn open(state: ReviewState) {
    if state.item.with_untracked(Option::is_none) {
        return;
    }
    crate::review::review_menu::close(state, false);
    crate::ui::agents::dropdown::close(state, false);
    crate::ui::usage::close(state, false);
    crate::ui::feedback::close(state, false);
    if !state.merge_open.get_untracked() {
        state.merge_open.set(true);
    }
}

/// Shut the merge box.
pub(crate) fn close(state: ReviewState) {
    state.merge_open.set(false);
}

fn toggle(state: ReviewState) {
    if state.merge_open.get_untracked() {
        close(state);
    } else {
        open(state);
    }
}

/// Merge as two parts on one pill: the method's name, which opens the
/// box, and a chevron for the methods, when there is more than one. Green
/// when the pull request could go in now; quiet when not, the box saying
/// why.
fn split_button(
    app: AppState,
    merge: MergeBox,
    state: ReviewState,
    bar: Memo<Option<BarMerge>>,
    main_id: RwSignal<Option<floem::ViewId>>,
) -> impl IntoView {
    let merging = move || {
        bar.with(|bar| match bar {
            Some(BarMerge::Merge { label, ready, pick }) => (label.clone(), *ready, *pick),
            _ => (String::new(), false, false),
        })
    };
    let roomy = Memo::new(move |_| {
        let drawer = if app.sidebar_open.get() {
            app.sidebar_width.get()
        } else {
            0.0
        };
        app.window_width.get() - drawer >= FULL_LABEL_ROOM
    });
    let label = move || {
        let (label, ..) = merging();
        if roomy.get() || state.merge_pending.get() {
            label
        } else {
            "Merge".to_string()
        }
    };
    let ready = Memo::new(move |_| merging().1);
    let pick = Memo::new(move |_| merging().2);
    let ink = move || {
        if ready.get() {
            theme::on_merge()
        } else {
            theme::fg()
        }
    };
    let chevron_ink = move || {
        if ready.get() {
            theme::on_merge()
        } else {
            // The label's ink: anything lighter is lost on the grey.
            theme::fg()
        }
    };

    let main = Container::new(Label::derived(label).style(move |s| {
        s.font_size(theme::TEXT_META)
            .font_weight(theme::weight_emphasis())
            .color(ink())
            .selectable(false)
            .pointer_events_none()
    }));
    main_id.set(Some(main.view_id()));
    let main = main
        .on_event_stop(el::Click, move |_, _| toggle(state))
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Enter | Code::Space) {
                toggle(state);
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .debug_name("merge-button")
        .style(|s| {
            clickable(s)
                .height_full()
                .padding_horiz(12.0)
                .items_center()
                .border_radius(9.0)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
        });

    let chevron = Container::new(
        sidebar_glyph(SidebarIcon::ChevronDown, chevron_ink)
            .style(|s| s.size(11.0, 11.0).pointer_events_none()),
    )
    .dropdown(move || merge.method_menu(state))
    .debug_name("merge-method-button")
    .style(move |s| {
        clickable(s)
            .height_full()
            .padding_horiz(9.0)
            .items_center()
            .border_left(1.0)
            .border_color(if ready.get() {
                theme::merge_rule()
            } else {
                theme::border()
            })
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .apply_if(!pick.get(), |s| s.hide())
    });

    Stack::horizontal((main, chevron))
        .debug_name("merge-split")
        .style(move |s| {
            let s = s.height(30.0).items_center().border_radius(9.0);
            if ready.get() {
                s.background(theme::merge_bg())
                    .hover(|s| s.background(theme::merge_hover()))
            } else {
                let open = state.merge_open.get();
                s.background(if open {
                    theme::border()
                } else {
                    theme::badge_bg()
                })
                .hover(|s| s.background(theme::border()))
            }
        })
}

/// Ready for review, on a draft the reader may mark ready: it does that,
/// and says so while it goes.
fn ready_button(
    state: ReviewState,
    bar: Memo<Option<BarMerge>>,
    backend: Rc<Backend>,
) -> impl IntoView {
    let pending =
        move || bar.with(|bar| matches!(bar, Some(BarMerge::ReadyForReview { pending: true })));
    // Nothing while a change to or from a draft is under way, whichever
    // way it goes.
    let act = move || {
        if state.draft_pending.with_untracked(Option::is_none) {
            crate::review::summary::set_draft(state, &backend, false);
        }
    };
    let key_act = act.clone();
    Container::new(
        Label::derived(move || {
            if pending() {
                "Marking ready…".to_string()
            } else {
                "Ready for review".to_string()
            }
        })
        .style(|s| {
            s.font_size(theme::TEXT_META)
                .font_weight(theme::weight_emphasis())
                .color(theme::fg())
                .selectable(false)
                .pointer_events_none()
        }),
    )
    .on_event_stop(el::Click, move |_, _| act())
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if matches!(key.code, Code::Enter | Code::Space) {
            key_act();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name("ready-for-review-button")
    .style(|s| {
        clickable(s)
            .height(30.0)
            .padding_horiz(12.0)
            .items_center()
            .border_radius(9.0)
            .background(theme::badge_bg())
            .hover(|s| s.background(theme::border()))
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
    })
}

/// The merge box, under the button: a heading naming where the pull
/// request goes, then the box. `quiet` when it opened on its own, and
/// then it leaves the keyboard where it is.
fn popover(
    state: ReviewState,
    backend: Rc<Backend>,
    merge: MergeBox,
    main_id: RwSignal<Option<floem::ViewId>>,
    quiet: bool,
) -> impl IntoView {
    let shut = move || {
        close(state);
        if let Some(id) = main_id.get_untracked() {
            id.request_focus();
        }
    };
    let close_button = icon_action(SidebarIcon::Close, "merge-close", || true, shut);
    // The keyboard goes to the close control, so Escape works at once.
    let close_id = close_button.view_id();
    let asked = Rc::new(Cell::new(quiet));
    let close_button = close_button.on_event_cont(el::UpdatePhaseLayout, move |_, _| {
        if !asked.replace(true) {
            close_id.request_focus();
        }
    });
    let into = state
        .pull_request
        .with_untracked(|pull| {
            pull.as_ref().map(|pull| {
                format!(
                    "#{} into {} from {}",
                    pull.number, pull.base_ref, pull.head_ref
                )
            })
        })
        .unwrap_or_default();

    let content = Stack::vertical((
        Stack::horizontal((
            Stack::vertical((
                Label::derived(|| "Merge".to_string()).style(|s| {
                    s.font_size(theme::TEXT_BODY)
                        .font_weight(theme::weight_emphasis())
                        .color(theme::fg())
                }),
                Label::derived(move || into.clone()).style(|s| {
                    s.font_size(theme::TEXT_META)
                        .color(theme::fg_subtle())
                        .text_ellipsis()
                        .min_width(0.0)
                        .margin_top(2.0)
                }),
            ))
            .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0_f32)),
            close_button,
        ))
        .style(|s| s.items_start().width_full().gap(8.0)),
        merge.section(state, backend),
    ))
    .style(|s| s.flex_col().width_full().gap(14.0));

    Container::new(content)
        .debug_name("merge-popover")
        .style(|s| {
            s.flex_col()
                .width(420.0)
                .max_width_full()
                .padding(14.0)
                .apply(crate::ui::menu::panel_surface(floem::style::Style::new()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use floem::headless::HeadlessHarness;
    use floem::peniko::Brush;
    use floem::prelude::{Key, KeyState, Modifiers, NamedKey};
    use floem::style::Background;
    use perch_github::{
        CheckState, Checks, MergeMethods, MergeState, MergeableState, PullRequestOverview,
        RepositoryPermission, ReviewDecision,
    };

    use crate::review::header::tests::{
        issue_harness, named, pull_request, pull_request_harness_with, rect, within,
    };

    /// GitHub's overview of pull request 812 at its head `h`: the reader
    /// may write, nothing conflicts, every method is allowed, and the
    /// rules are met.
    fn overview() -> PullRequestOverview {
        PullRequestOverview {
            head_sha: "h".into(),
            mergeable: MergeableState::Mergeable,
            merge_state: MergeState::Clean,
            review_decision: Some(ReviewDecision::Approved),
            viewer_can_merge_as_admin: false,
            viewer_permission: Some(RepositoryPermission::Write),
            allowed_merge_methods: MergeMethods {
                merge: true,
                squash: true,
                rebase: true,
            },
            checks: Checks {
                state: CheckState::Passing,
                url: None,
                runs: Vec::new(),
            },
            squash_message: Default::default(),
            merge_message: Default::default(),
            cross_repository: false,
            maintainer_can_modify: false,
        }
    }

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
    }

    /// The window on pull request 812 as `pull` has it, with `overview`.
    fn window(
        pull: perch_github::PullRequest,
        overview: Option<PullRequestOverview>,
    ) -> (HeadlessHarness, AppState, ReviewState) {
        let (mut harness, app, state) = pull_request_harness_with(pull);
        state.pull_request_overview.set(overview.map(Arc::new));
        settle(&mut harness);
        (harness, app, state)
    }

    fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        named(harness.root_id(), name).is_some_and(|id| id.get_visual_rect_no_clip().width() > 0.0)
    }

    fn label(harness: &HeadlessHarness, text: &str) -> bool {
        named(harness.root_id(), &format!("Label: \"{text}\"")).is_some()
    }

    fn background(harness: &HeadlessHarness, name: &str) -> Option<floem::peniko::Color> {
        let id = named(harness.root_id(), name).unwrap_or_else(|| panic!("no {name}"));
        match harness.get_computed_style(id).get(Background) {
            Some(Brush::Solid(color)) => Some(color),
            _ => None,
        }
    }

    fn click(harness: &mut HeadlessHarness, name: &str) {
        let at = rect(harness, name).center();
        harness.click(at.x, at.y);
        settle(harness);
    }

    fn key(code: Code, named: NamedKey) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Named(named),
            code,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    /// On an open pull request GitHub would merge, the bar has a green
    /// Merge named for the method, just left of Review; it opens the
    /// merge box under it, where the merge is confirmed.
    #[test]
    fn a_pull_request_that_can_go_in_has_a_green_merge_that_opens_the_box() {
        let (mut harness, app, state) = window(pull_request(812), Some(overview()));
        let bar = rect(&harness, "item bar");
        let split = rect(&harness, "merge-split");
        let review = rect(&harness, "review-control");
        let start = rect(&harness, "start work button");
        assert!(within(split, bar), "Merge at {split:?} is not in the bar");
        assert!(split.width() > 0.0);
        assert!(start.x1 <= split.x0 && split.x1 <= review.x0);
        assert!(
            review.x0 - split.x1 <= 8.0,
            "Merge at {split:?} is not beside Review at {review:?}"
        );
        // It costs the title room, not the switch its place at the centre:
        // plain Merge with the drawer open, named for the method with the
        // room the drawer leaves.
        let centred = |harness: &HeadlessHarness| {
            let (bar, switch) = (rect(harness, "item bar"), rect(harness, "item switch"));
            assert!(
                (switch.center().x - bar.center().x).abs() <= 1.0,
                "Merge pushed the switch at {switch:?} off the bar's centre {bar:?}"
            );
        };
        centred(&harness);
        assert!(label(&harness, "Merge"));
        app.sidebar_open.set(false);
        settle(&mut harness);
        centred(&harness);
        assert!(label(&harness, "Squash and merge"));
        let split = rect(&harness, "merge-split");
        assert_eq!(background(&harness, "merge-split"), Some(theme::merge_bg()));
        assert!(shown(&harness, "merge-method-button"));

        click(&mut harness, "merge-button");
        assert!(
            state.merge_open.get_untracked(),
            "the click did not open it"
        );
        let popover = rect(&harness, "merge-popover");
        assert!(
            popover.y0 >= split.y1 && popover.x1 == split.x1,
            "the box at {popover:?} does not hang from Merge at {split:?}"
        );
        assert!(popover.height() > 0.0);

        // The box's own merge asks to be confirmed, with the commit's
        // title and message to edit.
        click(&mut harness, "merge-action:Squash-and-merge");
        assert!(named(harness.root_id(), "merge-action:Confirm-merge").is_some());
        assert!(named(harness.root_id(), "merge-commit-title").is_some());

        click(&mut harness, "merge-close");
        assert!(!state.merge_open.get_untracked());
        assert!(named(harness.root_id(), "merge-popover").is_none());
        // Shutting it put the confirmation away.
        click(&mut harness, "merge-button");
        assert!(named(harness.root_id(), "merge-action:Confirm-merge").is_none());
    }

    /// The chevron's menu picks another of the methods the repository
    /// allows, and the button is named for it.
    #[test]
    fn the_method_menu_renames_the_button() {
        let (mut harness, app, _) = window(pull_request(812), Some(overview()));
        app.sidebar_open.set(false);
        settle(&mut harness);
        click(&mut harness, "merge-method-button");
        assert!(crate::ui::menu::is_open());
        harness.dispatch_event(key(Code::ArrowDown, NamedKey::ArrowDown));
        harness.dispatch_event(key(Code::Enter, NamedKey::Enter));
        settle(&mut harness);
        assert!(!crate::ui::menu::is_open());
        assert!(label(&harness, "Create a merge commit"));
        assert!(!label(&harness, "Squash and merge"));

        // One method allowed leaves nothing to pick.
        let mut one = overview();
        one.allowed_merge_methods = MergeMethods {
            merge: false,
            squash: false,
            rebase: true,
        };
        let (mut harness, app, _) = window(pull_request(812), Some(one));
        app.sidebar_open.set(false);
        settle(&mut harness);
        assert!(label(&harness, "Rebase and merge"));
        assert!(!shown(&harness, "merge-method-button"));
    }

    /// Blocked, by conflicts or by the rules, Merge stays but is quiet,
    /// and its box says why. The Status row's Conflicts opens the box at
    /// its resolver.
    #[test]
    fn a_blocked_pull_request_has_a_quiet_merge_and_conflicts_open_the_box() {
        let mut conflicting = overview();
        conflicting.mergeable = MergeableState::Conflicting;
        conflicting.merge_state = MergeState::Dirty;
        let (mut harness, _, state) = window(pull_request(812), Some(conflicting));
        assert!(shown(&harness, "merge-button"));
        assert_eq!(background(&harness, "merge-split"), Some(theme::badge_bg()));

        click(&mut harness, "readiness Conflicts");
        assert!(
            state.merge_open.get_untracked(),
            "Conflicts did not open the box"
        );
        assert!(label(
            &harness,
            "This branch has conflicts that must be resolved"
        ));
        // Its resolver: an agent's here, or GitHub's.
        assert!(named(harness.root_id(), "merge-action:Resolve-conflicts").is_some());

        let mut blocked = overview();
        blocked.merge_state = MergeState::Blocked;
        blocked.review_decision = Some(ReviewDecision::ReviewRequested);
        let (mut harness, ..) = window(pull_request(812), Some(blocked));
        assert_eq!(background(&harness, "merge-split"), Some(theme::badge_bg()));
        click(&mut harness, "merge-button");
        assert!(label(&harness, "Merging is blocked"));
        assert!(label(&harness, "An approving review is still required."));
    }

    /// A draft the reader may mark ready has Ready for review in Merge's
    /// place, which marks it so.
    #[test]
    fn a_draft_has_ready_for_review_in_merges_place() {
        let mut draft = pull_request(812);
        draft.draft = true;
        let (mut harness, _, state) = window(draft, Some(overview()));
        assert!(shown(&harness, "ready-for-review-button"));
        assert!(named(harness.root_id(), "merge-button").is_none());
        click(&mut harness, "ready-for-review-button");
        assert_eq!(state.draft_pending.get_untracked(), Some(false));
        assert!(label(&harness, "Marking ready…"));
        assert!(!state.merge_open.get_untracked());
    }

    /// No Merge where there is nothing the reader may merge: a pull
    /// request merged or closed, one they may only read, offline, an
    /// issue, and before GitHub says who may.
    #[test]
    fn there_is_no_merge_where_the_reader_cannot_merge() {
        let gone = |harness: &HeadlessHarness, why: &str| {
            assert!(!shown(harness, "merge-control"), "Merge {why}");
            assert!(!shown(harness, "ready-for-review-button"), "Ready {why}");
        };
        for (pull_state, merged) in [("closed", false), ("closed", true)] {
            let mut pull = pull_request(812);
            pull.state = pull_state.into();
            pull.merged = merged;
            let (harness, ..) = window(pull, Some(overview()));
            gone(&harness, &format!("on a {pull_state} one, merged {merged}"));
        }
        let (harness, ..) = window(pull_request(812), None);
        gone(&harness, "before GitHub says who may");

        let mut reads = overview();
        reads.viewer_permission = Some(RepositoryPermission::Read);
        let (harness, ..) = window(pull_request(812), Some(reads));
        gone(&harness, "for a reader who can't write");

        let (mut harness, _, state) = window(pull_request(812), Some(overview()));
        assert!(shown(&harness, "merge-control"));
        state.offline.set(true);
        settle(&mut harness);
        gone(&harness, "offline");

        let (mut harness, _, state) = window(pull_request(812), Some(overview()));
        state
            .historical_review
            .set(Some(crate::review::summary::tests::snapshot()));
        settle(&mut harness);
        gone(&harness, "on an old review's snapshot");

        let (harness, ..) = issue_harness();
        gone(&harness, "on an issue");
    }

    /// GitHub's status read again, as every poll and push has it, leaves
    /// the box open, and Merge in the bar meanwhile. A new snapshot of the
    /// same head keeps the confirmation under way too; a new status
    /// generation puts it away, as merge.rs means it to, but not the box.
    #[test]
    fn a_status_refresh_leaves_the_box_open() {
        use crate::backend::Event;

        let (mut harness, app, state) = window(pull_request(812), Some(overview()));
        click(&mut harness, "merge-button");
        click(&mut harness, "merge-action:Squash-and-merge");
        assert!(named(harness.root_id(), "merge-action:Confirm-merge").is_some());
        let still_open = |harness: &HeadlessHarness, state: ReviewState, why: &str| {
            assert!(state.merge_open.get_untracked(), "{why} shut the box");
            assert!(shown(harness, "merge-button"), "Merge went with {why}");
            assert!(shown(harness, "merge-popover"), "the box went with {why}");
        };

        let item = state.item.get_untracked().unwrap();
        app.apply_with_review(
            Event::PullRequestUpdated {
                item_id: "pr".into(),
                item,
                pull_request: Arc::new(pull_request(812)),
                merge_attempt_id: None,
            },
            state,
        );
        settle(&mut harness);
        still_open(&harness, state, "a new snapshot");
        app.apply_with_review(
            Event::PullRequestOverviewReady {
                item_id: "pr".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 812,
                status_generation: 0,
                overview: Arc::new(overview()),
            },
            state,
        );
        settle(&mut harness);
        assert!(
            named(harness.root_id(), "merge-action:Confirm-merge").is_some(),
            "the snapshot put the confirmation away"
        );

        app.apply_with_review(
            Event::PullRequestOverviewLoading {
                item_id: "pr".into(),
                repo_full_name: "slatedb/slatedb".into(),
                number: 812,
                head_sha: "h".into(),
                status_generation: 1,
            },
            state,
        );
        settle(&mut harness);
        still_open(&harness, state, "a status refresh");
    }

    /// A merge that fails after the box was shut opens it to say why, but
    /// leaves the keyboard where the reader has it.
    #[test]
    fn a_failed_merge_opens_the_box_without_taking_the_keyboard() {
        let (mut harness, _, state) = window(pull_request(812), Some(overview()));
        click(&mut harness, "merge-button");
        click(&mut harness, "merge-close");
        assert!(!state.merge_open.get_untracked());
        let button = named(harness.root_id(), "merge-button").unwrap();
        assert!(
            harness.is_focused(button),
            "Merge does not have the keyboard"
        );

        state
            .merge_error
            .set(Some("Required status check is failing".into()));
        settle(&mut harness);
        assert!(state.merge_open.get_untracked(), "the failure is not shown");
        assert!(named(harness.root_id(), "Required status check is failing").is_some());
        assert!(
            harness.is_focused(button),
            "the box took the keyboard as it opened"
        );
    }

    /// The bar is the same on Code, and Merge with it.
    #[test]
    fn merge_is_in_the_bar_on_code_too() {
        let (mut harness, _, state) = window(pull_request(812), Some(overview()));
        click(&mut harness, "Code pill");
        assert_eq!(
            state.tab.get_untracked(),
            crate::review::model::ItemTab::Code
        );
        assert!(shown(&harness, "merge-button"));
        click(&mut harness, "merge-button");
        assert!(rect(&harness, "merge-popover").height() > 0.0);
    }

    /// Green and all, the bar goes dark with the system.
    #[test]
    fn the_merge_button_goes_dark_with_the_system() {
        let (mut harness, ..) = window(pull_request(812), Some(overview()));
        click(&mut harness, "merge-button");
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(
            left.is_empty(),
            "light colours left in dark:\n{}",
            left.join("\n")
        );
    }
}
