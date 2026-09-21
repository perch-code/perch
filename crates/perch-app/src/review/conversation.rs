//! Conversation: the item's description, everything said about it on
//! GitHub, and every thread the reader has had with an agent about it, in
//! one quiet reading column in time order, with the composer at the end.
//!
//! The order is `review::timeline`'s; this module only draws it. The cards
//! are in `review::cards`, and a thread's card shares its views with the
//! inline card on Files.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};

use crate::backend::Backend;
use crate::review::cards;
use crate::review::model::{ReviewFindingFilter, ReviewState};
use crate::review::threads::{self, ThreadFace};
use crate::review::timeline::{timeline, Card};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::clickable;

/// One scrolling column, centred, on the page grey.
pub fn view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    open_item: crate::ui::items::OpenItem,
) -> impl IntoView {
    let card_backend = backend.clone();
    let summary_backend = backend.clone();
    let composer_backend = backend.clone();
    // Each card with the room above it: cards sit the standard gap apart,
    // and an event line is pulled in toward whatever is on either side of
    // it. The gap is in the key, so a card whose neighbour changes kind
    // is laid out again.
    let cards = move || {
        let pull = state.pull_request.get();
        let discussion = state.discussion.get();
        let issue = state.issue.get();
        let comments = state.issue_comments.get();
        let threads = state.comment_threads.with(|threads| {
            threads
                .iter()
                .filter(|thread| state.thread_visible(&thread.id))
                .cloned()
                .collect::<Vec<_>>()
        });
        let cards = timeline(
            pull.as_deref(),
            discussion.as_deref(),
            issue.as_deref().map(|i| (i, comments.as_slice())),
            &threads,
        );
        let mut rows = Vec::with_capacity(cards.len());
        let mut previous: Option<&Card> = None;
        for card in &cards {
            let gap = match previous {
                None => 0.0,
                Some(p) if matches!(p, Card::Event(_)) || matches!(card, Card::Event(_)) => {
                    theme::EVENT_GAP
                }
                Some(_) => theme::CARD_GAP,
            };
            rows.push((card.clone(), gap));
            previous = Some(card);
        }
        rows
    };
    let empty = move || {
        // Only a bare branch has nothing: a pull request or an issue has a
        // description whatever else it has.
        state.comment_threads.with(|t| t.is_empty())
            && state.pull_request.with(Option::is_none)
            && state.issue.with(Option::is_none)
            && state
                .item
                .with(|item| item.as_ref().map(|item| item.id.clone()))
                .is_none_or(|item_id| app.review_groups_for(&item_id).is_empty())
    };

    Container::new(
        Stack::vertical((
            conversation_title(state),
            crate::review::summary::summary(app, state, summary_backend, open_item),
            finding_filters(app, state),
            dyn_stack(
                cards,
                |(card, gap)| format!("{}:{gap}", cards::key(card)),
                move |(card, gap)| {
                    card_view(
                        app,
                        state,
                        card_backend.clone(),
                        open_file.clone(),
                        card,
                        gap,
                    )
                },
            )
            .style(|s| s.flex_col().width_full()),
            crate::review::review_launch::history(app, state, backend.clone()),
            crate::review::merge::surface(state, backend.clone()),
            Label::derived(|| "Nothing here yet. Ask about the branch below.".to_string()).style(
                move |s| {
                    // Hidden rather than absent, so it takes no share of the
                    // column's gap while there is something here.
                    s.font_size(theme::TEXT_BODY)
                        .color(theme::FG_SUBTLE)
                        .width_full()
                        .text_align(floem::text::Alignment::Center)
                        .padding_vert(28.0)
                        .selectable(false)
                        .apply_if(!empty(), |s| s.hide())
                },
            ),
            composer(app, state, composer_backend).style(|s| {
                s.width_full()
                    .padding_top(theme::CONVERSATION_ENTRY_PADDING)
            }),
        ))
        // A definite width, cut to the page's when the page is narrower,
        // rather than the page's width capped: under a cap, the pass that
        // sizes the page unconstrained lands on the cap's width as well,
        // and taffy takes its unwrapped height for the constrained pass
        // at that same width. The column's bottom, wrapped rows and long
        // comments included, could then not be scrolled into view.
        .style(|s| {
            s.flex_col()
                .width(theme::CONVERSATION_WIDTH)
                .max_width_full()
        }),
    )
    .style(|s| {
        s.width_full()
            .items_start()
            .justify_center()
            .padding(theme::CONVERSATION_PADDING)
    })
    .scroll()
    .debug_name("conversation")
    // A bare key goes to the focused view and bubbles up from there, and
    // a click on the page would otherwise focus nothing: the column takes
    // the focus so `a` reaches the window's handler.
    .style(|s| {
        s.size_full()
            .min_height(0.0)
            .background(theme::CANVAS)
            .focusable()
    })
}

fn finding_filters(app: AppState, state: ReviewState) -> impl IntoView {
    dyn_container(
        move || {
            let item_id = state
                .item
                .with(|item| item.as_ref().map(|item| item.id.clone()))?;
            let findings = state
                .historical_findings
                .get()
                .unwrap_or_else(|| app.review_findings_for(&item_id));
            (!findings.is_empty()).then_some((item_id, findings))
        },
        move |value| match value {
            Some((item_id, findings)) => {
                let open = findings
                    .iter()
                    .filter(|finding| finding.is_actionable())
                    .count();
                let total = findings.len();
                let open_findings = findings.clone();
                let all_findings = findings;
                Stack::horizontal((
                    Label::derived(move || format!("Review findings · {open} open")).style(|s| {
                        s.font_size(theme::TEXT_META)
                            .font_weight(FontWeight::MEDIUM)
                            .color(theme::FG_MUTED)
                            .margin_right(4.0)
                    }),
                    finding_filter_button("Open", ReviewFindingFilter::Open, state, open_findings),
                    finding_filter_button("All", ReviewFindingFilter::All, state, all_findings),
                    Label::derived(move || format!("{total} total"))
                        .style(|s| s.font_size(theme::TEXT_BADGE).color(theme::FG_SUBTLE)),
                ))
                .debug_name(format!("review-finding-filters:{item_id}"))
                .style(|s| {
                    s.items_center()
                        .gap(6.0)
                        .padding_bottom(theme::CONVERSATION_ENTRY_PADDING)
                        .border_bottom(1.0)
                        .border_color(theme::BORDER_MUTED)
                })
                .into_any()
            }
            None => Empty::new().into_any(),
        },
    )
}

fn finding_filter_button(
    label: &'static str,
    filter: ReviewFindingFilter,
    state: ReviewState,
    findings: std::sync::Arc<Vec<crate::review::model::ReviewFindingSummary>>,
) -> impl IntoView {
    Label::derived(move || label.to_string())
        .on_event_stop(el::Click, move |_, _| {
            state.apply_review_finding_filter(filter, &findings);
        })
        .style(move |s| {
            clickable(s)
                .height(24.0)
                .padding_horiz(8.0)
                .items_center()
                .font_size(theme::TEXT_BADGE)
                .color(if state.review_finding_filter.get() == filter {
                    theme::FG
                } else {
                    theme::FG_MUTED
                })
                .background(if state.review_finding_filter.get() == filter {
                    theme::BADGE_BG
                } else {
                    theme::CANVAS
                })
                .border_radius(theme::RADIUS)
                .cursor(CursorStyle::Pointer)
                .selectable(false)
        })
}

/// The full item title belongs in the reading surface as well as the compact
/// window bar. This mirrors Codex: the bar orients, while the page title starts
/// the document at a comfortable reading size.
fn conversation_title(state: ReviewState) -> impl IntoView {
    let title = move || {
        state
            .pull_request
            .with(|p| p.as_ref().map(|p| p.title.clone()))
            .or_else(|| {
                state
                    .item
                    .with(|i| i.as_ref().map(|i| i.title().to_string()))
            })
            .unwrap_or_default()
    };
    Label::derived(title)
        .debug_name("conversation-title")
        .style(|s| {
            s.width_full()
                .font_size(theme::TEXT_PAGE_TITLE)
                .font_weight(FontWeight::MEDIUM)
                .line_height(1.18)
                .color(theme::FG)
                .padding_bottom(theme::CONVERSATION_TITLE_GAP)
        })
}

/// One card, with the room above it.
fn card_view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
    card: Card,
    gap: f64,
) -> floem::AnyView {
    let pull = state.pull_request.get_untracked();
    let debug = card.kind();
    let view: floem::AnyView = match card {
        // What Perch started from an issue is the summary's In Perch row,
        // not repeated under the description.
        Card::Description {
            author,
            avatar_url,
            at,
            body,
            node_id,
        } => cards::description_card(app, state, backend, author, avatar_url, at, body, node_id)
            .into_any(),
        Card::Comment(comment) => cards::comment_card(app, state, backend, comment).into_any(),
        Card::Review { review, comments } => {
            cards::review_card(app, state, backend, open_file, review, comments).into_any()
        }
        Card::Event(event) => {
            cards::event_row(app, event, pull.map(|p| p.author_login.clone())).into_any()
        }
        Card::Thread(thread) => {
            cards::thread_card(app, state, backend, open_file, thread).into_any()
        }
    };
    view.debug_name(format!("card:{debug}"))
        .style(move |s| s.width_full().margin_top(gap))
        .into_any()
}

/// The composer for asking about the item as a whole, in place at the end
/// of the column, or the box that opens it. A follow-up's composer is
/// under its thread instead.
fn composer(app: AppState, state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    // Keyed on whether one is open, what it is about and which opening it
    // is, not on the composer itself: picking a model writes the composer,
    // and a rebuild on that would throw the typed question away. The
    // opening count is there so Ask about this on a second comment, which
    // opens a composer about the same thing with another quote, gets a
    // fresh box rather than the first one's text.
    let open = floem::reactive::Memo::new(move |_| {
        state.composer.with(|c| {
            c.as_ref()
                .filter(|c| c.thread_id.is_none())
                .map(|c| (c.anchor.clone(), c.opening))
        })
    });
    dyn_container(
        move || open.get(),
        move |open| match open {
            Some(_) => match state.composer.get_untracked() {
                Some(c) => threads::composer_view(app, state, backend.clone(), c, ThreadFace::Card)
                    .into_any(),
                None => Empty::new().into_any(),
            },
            None => cards::ask_box(app, state).into_any(),
        },
    )
    .debug_name("composer")
    .style(|s| s.width_full())
}

/// The whole window with an item open on Conversation, and what is on it.
#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_github::{
        Anchor, Author, CommentMessage, CommentThread, Discussion, IssueComment, Item,
        ItemCheckout, ItemSubject, PullEvent, PullEventKind, PullRequest, Review, ReviewComment,
        RunRecord, RunStatus, RunStep, Side, ThreadOrigin,
    };

    use crate::review::model::ReviewState;
    use crate::review::threads::ThreadFace;
    use crate::state::AppState;

    fn at(s: &str) -> jiff::Timestamp {
        s.parse().unwrap()
    }

    fn pull() -> PullRequest {
        PullRequest {
            number: 57,
            title: "Fence the manifest".into(),
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "fence".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: true,
            state: "closed".into(),
            additions: 12,
            deletions: 3,
            changed_files: 1,
            repo_full_name: "criccomini/perch".into(),
            html_url: "https://github.com/criccomini/perch/pull/57".into(),
            head_repo_full_name: None,
            updated_at: None,
            review_requested: false,
            checks: None,
            review_decision: None,
            merged_at: Some(at("2026-09-17T15:00:00Z")),
            merged_by: Some("rohan".into()),
            closed_at: Some(at("2026-09-17T15:00:00Z")),
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        }
    }

    /// One comment, one approving review with a line comment and a reply,
    /// and the merge.
    fn discussion() -> Discussion {
        let line = |id: u64, reply_to: Option<u64>, when: &str, body: &str| ReviewComment {
            node_id: String::new(),
            reactions: Default::default(),
            id,
            pull_request_review_id: Some(9),
            in_reply_to_id: reply_to,
            user: "chris".into(),
            body: body.into(),
            path: "src/manifest.rs".into(),
            line: Some(41),
            original_line: None,
            diff_hunk: "@@ -39,3 +39,3 @@\n a\n-b\n+c".into(),
            side: Side::Right,
            created_at: at(when),
            updated_at: None,
            html_url: format!("https://github.com/criccomini/perch/pull/57#discussion_r{id}"),
        };
        Discussion {
            node_id: String::new(),
            reactions: Default::default(),
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            comments: vec![IssueComment {
                node_id: String::new(),
                reactions: Default::default(),
                id: 1,
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "Thanks".into(),
                created_at: at("2026-09-15T10:00:00Z"),
                updated_at: None,
                html_url: "https://github.com/criccomini/perch/pull/57#issuecomment-1".into(),
            }],
            reviews: vec![Review {
                node_id: String::new(),
                reactions: Default::default(),
                id: 9,
                user: "chris".into(),
                body: "Looks right.".into(),
                state: "APPROVED".into(),
                submitted_at: Some(at("2026-09-16T11:40:00Z")),
                html_url: String::new(),
            }],
            review_comments: vec![
                line(31, None, "2026-09-16T11:38:00Z", "epoch"),
                line(32, Some(31), "2026-09-16T12:00:00Z", "the reply"),
            ],
            events: vec![
                PullEvent {
                    kind: PullEventKind::Opened,
                    at: at("2026-09-14T14:02:00Z"),
                },
                PullEvent {
                    kind: PullEventKind::Merged {
                        by: Some("rohan".into()),
                    },
                    at: at("2026-09-17T15:00:00Z"),
                },
            ],
        }
    }

    fn run() -> RunRecord {
        RunRecord {
            id: "r1".into(),
            owner: perch_github::RunOwner::Discussion {
                item_id: "wt".into(),
                thread_id: "t1".into(),
                request_id: "q1".into(),
            },
            backend: "claude-code".into(),
            model: "sonnet".into(),
            effort: "high".into(),
            session_id: Some("sess".into()),
            status: RunStatus::Completed,
            cost_usd: Some(0.03),
            duration_ms: Some(14_000),
            steps: vec![
                RunStep {
                    summary: "Read src/manifest.rs".into(),
                    ok: true,
                },
                RunStep {
                    summary: "Ran git log".into(),
                    ok: true,
                },
            ],
            error: None,
            started_at: Some(at("2026-09-16T13:00:00Z")),
            ended_at: Some(at("2026-09-16T13:00:14Z")),
            ..RunRecord::default()
        }
    }

    /// A thread asked between the review and the merge.
    fn thread() -> CommentThread {
        let message = |id: &str, author: Author, body: &str, when: &str| CommentMessage {
            id: id.into(),
            thread_id: "t1".into(),
            origin: ThreadOrigin::Local,
            author,
            body: body.into(),
            created_at: at(when),
            run: Some(run()),
        };
        CommentThread {
            id: "t1".into(),
            worktree_id: "wt".into(),
            origin: ThreadOrigin::Local,
            anchor: Some(Anchor {
                path: "src/manifest.rs".into(),
                start_line: 41,
                line: 58,
                side: Side::Right,
                commit_sha: "h".into(),
            }),
            outdated: false,
            resolved: false,
            created_at: at("2026-09-16T13:00:00Z"),
            messages: vec![
                message("m1", Author::User, "why the fence?", "2026-09-16T13:00:00Z"),
                message(
                    "m2",
                    Author::Agent {
                        backend: "claude-code".into(),
                        model: "sonnet".into(),
                    },
                    "because",
                    "2026-09-16T13:00:14Z",
                ),
            ],
        }
    }

    fn app_harness() -> (HeadlessHarness, AppState, ReviewState) {
        app_harness_on(crate::backend::Backend::detached(), discussion())
    }

    /// The window with the pull request open on Conversation, with this
    /// discussion, and the backend the window's asks go to.
    fn app_harness_on(
        backend: crate::backend::Backend,
        discussion: Discussion,
    ) -> (HeadlessHarness, AppState, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let pr = pull();
        let now = at("2026-09-18T09:59:00Z");
        let mut item = Item::new("criccomini/perch", now);
        item.id = "wt".into();
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        item.checkout = Some(ItemCheckout {
            branch: "perch/pr-57".into(),
            base_ref: "main".into(),
            head_sha: "h".into(),
            path: std::path::PathBuf::from("/tmp/wt"),
            owned: true,
            on_disk: true,
            churn: None,
            last_seen_head: None,
        });
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(crate::review::model::Phase::Ready);
        state.pull_request.set(Some(Arc::new(pr)));
        state.install_reactions(discussion.reactions_by_id());
        state.discussion.set(Some(Arc::new(discussion)));
        state.comment_threads.set(Arc::new(vec![thread()]));
        // See the field: a focused editor cannot be driven headlessly.
        state.composer_autofocus.set(false);
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
    }

    /// The reactions on a comment are pills under it, one a kind, and a
    /// click on one toggles it where it is and asks the backend; a second
    /// click waits for the answer. An entry nobody reacted to has no row,
    /// and one an older build kept without a node id has no picker.
    #[test]
    fn a_reaction_pill_toggles_where_it_is_and_asks_the_backend() {
        use perch_github::{ReactionGroup, ReactionKind, Reactions};
        let (backend, mut commands) = crate::backend::Backend::recording();
        let mut discussion = discussion();
        discussion.node_id = "PR_57".into();
        discussion.comments[0].node_id = "IC_1".into();
        discussion.comments[0].reactions = Reactions {
            groups: vec![
                ReactionGroup { kind: ReactionKind::ThumbsUp, count: 2, viewer_reacted: false },
                ReactionGroup { kind: ReactionKind::Rocket, count: 1, viewer_reacted: true },
            ],
        };
        let (mut harness, app, state) = app_harness_on(backend, discussion);
        let root = harness.root_id();
        let comment = find(root, "card:comment").unwrap();
        let description = find(root, "card:description").unwrap();
        let review = find(root, "card:review").unwrap();

        // The pills, and the picker at their end; the description, with
        // nothing on it, has the row hidden; the review, kept without a
        // node id, has no picker at all.
        let pill = |harness: &HeadlessHarness, kind: &str| {
            let id = find(comment, &format!("reaction:{kind}")).unwrap_or_else(|| panic!("no {kind} pill"));
            let mut all = Vec::new();
            names(id, &mut all);
            (id, all.join(" "), harness.get_layout_rect(id).height())
        };
        let (thumbs, text, height) = pill(&harness, "THUMBS_UP");
        assert!(text.contains("\"2\""), "{text}");
        assert_eq!(height, crate::theme::REACTION_PILL_HEIGHT);
        assert!(pill(&harness, "ROCKET").1.contains("\"1\""));
        assert!(find(comment, "reaction:EYES").is_none(), "nobody left one");
        assert!(find(comment, "react-button").is_some());
        let row = find(description, "reactions").unwrap();
        assert_eq!(harness.get_layout_rect(row).height(), 0.0, "nothing to show");
        assert!(find(description, "react-button").is_some(), "the picker is the way in");
        assert!(find(review, "react-button").is_none());
        assert!(find(review, "reactions").is_none());

        // A click: the pill counts the reader in before GitHub is asked.
        let c = thumbs.get_visual_rect_no_clip().center();
        assert_eq!(
            harness.element_id_at(c.x, c.y).map(|e| e.owning_id()),
            Some(thumbs),
            "the pill is not under the pointer"
        );
        // Whatever the window asked for on its way up is not the click's.
        while commands.try_recv().is_ok() {}
        harness.click(c.x, c.y);
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::SetReaction { item_id, node_id, kind: ReactionKind::ThumbsUp, add: true })
                if item_id == "wt" && node_id == "IC_1"
        ));
        assert!(state.reactions_of("IC_1").viewer_reacted(ReactionKind::ThumbsUp));
        harness.rebuild();
        harness.paint();
        let (thumbs, text, _) = pill(&harness, "THUMBS_UP");
        assert!(text.contains("\"3\""), "{text}");

        // A second click on the same pill waits for GitHub.
        let c = thumbs.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        assert!(commands.try_recv().is_err(), "asked twice");
        assert_eq!(state.reactions_of("IC_1").count(ReactionKind::ThumbsUp), 3);

        // GitHub's answer is what the row shows then, and the pill is a
        // pill again.
        app.apply_with_review(
            crate::backend::Event::ReactionsUpdated {
                item_id: "wt".into(),
                node_id: "IC_1".into(),
                kind: ReactionKind::ThumbsUp,
                reactions: Reactions {
                    groups: vec![ReactionGroup { kind: ReactionKind::ThumbsUp, count: 4, viewer_reacted: true }],
                },
            },
            state,
        );
        harness.rebuild();
        harness.paint();
        let (thumbs, text, _) = pill(&harness, "THUMBS_UP");
        assert!(text.contains("\"4\""), "{text}");
        assert!(find(comment, "reaction:ROCKET").is_none(), "GitHub says nobody's");
        let c = thumbs.get_visual_rect_no_clip().center();
        harness.click(c.x, c.y);
        assert!(matches!(
            commands.try_recv(),
            Ok(crate::backend::Command::SetReaction { kind: ReactionKind::ThumbsUp, add: false, .. })
        ));
        assert_eq!(state.reactions_of("IC_1").count(ReactionKind::ThumbsUp), 3);
    }

    /// Every view's debug names, in tree order.
    fn names(id: floem::ViewId, out: &mut Vec<String>) {
        out.push(id.debug_name());
        for child in id.children() {
            names(child, out);
        }
    }

    /// The first view whose debug name contains `text`.
    fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
        if id.debug_name().contains(text) {
            return Some(id);
        }
        id.children().into_iter().find_map(|c| find(c, text))
    }

    /// Whether a view is laid out at all: a hidden one has no size.
    fn shown(harness: &HeadlessHarness, text: &str) -> bool {
        find(harness.root_id(), text).is_some_and(|id| harness.get_layout_rect(id).height() > 0.0)
    }

    fn cards_in_order(harness: &HeadlessHarness) -> Vec<String> {
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        all.into_iter()
            .filter_map(|n| {
                n.split_whitespace()
                    .find(|w| w.starts_with("card:"))
                    .map(str::to_string)
            })
            .collect()
    }

    /// The page's end can be scrolled to, whatever wraps on it. Under a
    /// width cap taffy served the column's unwrapped height for the pass
    /// at the cap's width, and the composer under long comments or a
    /// wrapped summary row sat past the scroll's end.
    #[test]
    fn the_end_of_the_page_can_be_scrolled_into_view_when_its_text_and_rows_wrap() {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let mut pr = pull();
        pr.labels = (0..40)
            .map(|i| perch_github::Label {
                name: format!("label-{i}"),
                color: "0e8a16".into(),
            })
            .collect();
        let now = at("2026-09-18T09:59:00Z");
        let mut item = Item::new("criccomini/perch", now);
        item.id = "wt".into();
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(crate::review::model::Phase::Ready);
        state.pull_request.set(Some(Arc::new(pr)));
        let mut discussion = discussion();
        for i in 0..3 {
            discussion.comments.push(IssueComment {
                node_id: String::new(),
                reactions: Default::default(),
                id: 100 + i,
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "A sentence that runs on well past the column's width, ".repeat(12),
                created_at: at("2026-09-15T11:00:00Z"),
                updated_at: None,
                html_url: format!("https://github.com/criccomini/perch/pull/57#issuecomment-{i}"),
            });
        }
        state.discussion.set(Some(Arc::new(discussion)));
        state.composer_autofocus.set(false);
        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();

        // The column is at its full width, and its rows and comments
        // wrap.
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let column = find(harness.root_id(), "conversation-title")
            .unwrap()
            .parent()
            .unwrap();
        assert_eq!(
            harness.get_layout_rect(column).width(),
            crate::theme::CONVERSATION_WIDTH
        );
        let labels = find(harness.root_id(), "summary row Labels").unwrap();
        assert!(harness.get_layout_rect(labels).height() > crate::theme::SUMMARY_ROW);

        // What the scroll thinks it holds is what the column is, with the
        // page's padding around it.
        let child = scroll.children()[0];
        assert_eq!(
            harness.get_layout_rect(child).height(),
            harness.get_layout_rect(column).height() + 2.0 * crate::theme::CONVERSATION_PADDING,
            "the scroll's content is not the column"
        );

        // And so the composer, at the column's end, can be reached.
        let viewport = scroll.get_visual_rect_no_clip();
        harness.scroll_down(viewport.center().x, viewport.center().y, 100_000.0);
        harness.rebuild();
        harness.paint();
        let composer = find(harness.root_id(), "composer")
            .unwrap()
            .get_visual_rect_no_clip();
        assert_eq!(
            composer.y1,
            viewport.y1 - crate::theme::CONVERSATION_PADDING,
            "the composer at {composer:?} is not at the end of the scroll {viewport:?}"
        );
    }

    #[test]
    fn the_cards_are_in_time_order_with_the_thread_between_the_review_and_the_merge() {
        let (harness, _, state) = app_harness();
        assert_eq!(state.tab.get(), crate::review::model::ItemTab::Conversation);
        assert_eq!(
            cards_in_order(&harness),
            [
                "card:description",
                "card:event",
                "card:comment",
                "card:review",
                "card:thread",
                "card:event"
            ]
        );
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let text = all.join("\n");
        for expected in [
            "Thanks",
            "approved",
            "Local",
            "You",
            "merged",
            "src/manifest.rs:41",
            "Ask about this pull request",
        ] {
            assert!(text.contains(expected), "no {expected:?} on screen");
        }
        assert!(!shown(&harness, "Nothing here yet"));
    }

    /// A name on a card is a link to its page on GitHub: the author of a
    /// comment, and who opened the pull request on its event line. The
    /// reader's own thread says "You" until GitHub has said who they are,
    /// and "You" is no link, in look or in deed.
    #[test]
    fn a_name_on_a_card_is_a_link_to_its_page_on_github() {
        use crate::theme;
        use floem::peniko::Color;
        use floem::style::{BorderBottomColor, Cursor, CursorStyle};
        let (mut harness, app, _) = app_harness();
        let opened = crate::ui::widgets::opened;
        opened();
        // The name label under a card, and the click that reaches it. The
        // lookup is scoped to the card: chris also reviewed, so the strip
        // under the header names them first, and rohan wrote the
        // description as well as opening.
        let name_in = |card: floem::ViewId, name: &str| {
            find(card, &format!("name label - Label: {name:?}"))
                .unwrap_or_else(|| panic!("no name {name:?} on {}", card.debug_name()))
        };
        let click = |harness: &mut HeadlessHarness, id: floem::ViewId| {
            let c = id.get_visual_rect_no_clip().center();
            assert_eq!(
                harness.element_id_at(c.x, c.y).map(|e| e.owning_id()),
                Some(id),
                "{} is not under the pointer at {c:?}",
                id.debug_name()
            );
            harness.click(c.x, c.y);
        };
        let root = harness.root_id();
        let comment = find(root, "card:comment").unwrap();
        click(&mut harness, name_in(comment, "chris"));
        assert_eq!(opened(), vec!["https://github.com/chris".to_string()]);
        let event = find(root, "card:event").unwrap();
        click(&mut harness, name_in(event, "rohan"));
        assert_eq!(opened(), vec!["https://github.com/rohan".to_string()]);

        // The reader: "You" has no underline and no pointer, and leads
        // nowhere; once GitHub has named them, the same label is a link.
        let thread = find(root, "card:thread").unwrap();
        let you = name_in(thread, "You");
        // The summary over the cards puts the thread under the fold.
        you.scroll_to(None);
        harness.rebuild();
        harness.paint();
        let c = you.get_visual_rect_no_clip().center();
        harness.pointer_move(c.x, c.y);
        harness.recompute_styles();
        let plain = harness.get_computed_style(you);
        assert_eq!(
            plain.get(BorderBottomColor),
            Some(Color::TRANSPARENT.into())
        );
        assert_ne!(plain.get(Cursor), Some(CursorStyle::Pointer));
        click(&mut harness, you);
        assert_eq!(opened(), Vec::<String>::new(), "You led somewhere");
        app.viewer.set(Some(("me".into(), String::new())));
        harness.rebuild();
        harness.paint();
        let me = name_in(thread, "me");
        assert_eq!(me, you, "the label was rebuilt rather than reworded");
        harness.pointer_move(c.x, c.y);
        harness.recompute_styles();
        let linked = harness.get_computed_style(me);
        assert_eq!(linked.get(BorderBottomColor), Some(theme::FG_MUTED.into()));
        assert_eq!(linked.get(Cursor), Some(CursorStyle::Pointer));
        click(&mut harness, me);
        assert_eq!(opened(), vec!["https://github.com/me".to_string()]);
    }

    /// Conversation is the home for local threads that have no inline card:
    /// item-level threads, stale anchors, and anchors in another file.
    #[test]
    fn local_threads_without_an_inline_home_still_draw_on_conversation() {
        let (mut harness, _, state) = app_harness();
        let local_thread =
            |id: &str, body: &str, path: Option<&str>, outdated: bool| -> CommentThread {
                let mut thread = thread();
                thread.id = id.into();
                thread.anchor = match path {
                    Some(path) => {
                        let mut anchor = thread.anchor.take().unwrap();
                        anchor.path = path.into();
                        Some(anchor)
                    }
                    None => None,
                };
                thread.outdated = outdated;
                for (index, message) in thread.messages.iter_mut().enumerate() {
                    message.id = format!("{id}-m{index}");
                    message.thread_id = id.into();
                    message.run = None;
                }
                thread.messages[0].body = body.into();
                thread
            };
        state.comment_threads.set(Arc::new(vec![
            local_thread("item-level", "question about the whole item", None, false),
            local_thread(
                "outdated",
                "question about stale lines",
                Some("src/manifest.rs"),
                true,
            ),
            local_thread(
                "other-file",
                "question in another file",
                Some("src/elsewhere.rs"),
                false,
            ),
        ]));
        harness.rebuild();
        harness.paint();

        assert_eq!(
            cards_in_order(&harness)
                .iter()
                .filter(|card| card.as_str() == "card:thread")
                .count(),
            3
        );
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let text = all.join("\n");
        for expected in [
            "question about the whole item",
            "question about stale lines",
            "question in another file",
            "Outdated",
            "src/elsewhere.rs:41",
        ] {
            assert!(text.contains(expected), "no {expected:?} on Conversation");
        }
    }

    #[test]
    fn a_bare_branch_says_so_above_the_composer() {
        let (mut harness, _, state) = app_harness();
        state.pull_request.set(None);
        state.discussion.set(None);
        state.comment_threads.set(Arc::new(vec![]));
        harness.rebuild();
        harness.paint();
        assert!(cards_in_order(&harness).is_empty());
        assert!(shown(&harness, "Nothing here yet"));
        assert!(shown(&harness, "Ask about this branch"));
    }

    #[test]
    fn the_composer_opens_in_place_and_a_follow_up_under_its_thread() {
        let (mut harness, _, state) = app_harness();
        state.open_composer(
            None,
            None,
            "why?".into(),
            (
                perch_agent::Backend::ClaudeCode,
                "sonnet".into(),
                perch_agent::Effort::Medium,
            ),
        );
        harness.rebuild();
        harness.paint();
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        assert!(
            all.iter().any(|n| n.contains("composer")),
            "the in-place composer is not in the tree"
        );
        assert!(
            state.composer_box.get().is_some(),
            "the box did not register itself for `a`"
        );

        // Picking a model keeps the box: the container is keyed on what
        // the composer is about, not on the composer.
        let before = state.composer_box.get();
        state.composer.update(|c| {
            if let Some(c) = c {
                c.model = "opus".into();
            }
        });
        harness.rebuild();
        harness.paint();
        assert_eq!(state.composer_box.get(), before);

        state.close_composer();
        harness.rebuild();
        harness.paint();
        assert!(
            state.composer_box.get().is_none(),
            "closing the composer left its box registered"
        );
    }

    /// The thread views build for Conversation and inline, from the same thread.
    #[test]
    fn a_thread_draws_with_both_faces() {
        use floem::views::Stack;
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        state.comment_threads.set(Arc::new(vec![thread()]));
        let backend = Rc::new(crate::backend::Backend::detached());
        let open_file: crate::review::OpenFile = Rc::new(|_| {});
        let view = Stack::vertical((
            crate::review::threads::messages_view(
                app,
                state,
                backend.clone(),
                "t1".into(),
                ThreadFace::Card,
            ),
            crate::review::threads::messages_view(
                app,
                state,
                backend.clone(),
                "t1".into(),
                ThreadFace::Inline,
            ),
            crate::review::threads::follow_up(
                app,
                state,
                backend.clone(),
                "t1".into(),
                ThreadFace::Card,
            ),
            crate::review::threads::follow_up(
                app,
                state,
                backend.clone(),
                "t1".into(),
                ThreadFace::Inline,
            ),
            crate::review::cards::thread_card(
                app,
                state,
                backend.clone(),
                open_file.clone(),
                thread(),
            ),
            crate::review::threads::thread_card(app, state, backend, thread()),
        ));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 1600.0);
        harness.rebuild();
        harness.paint();
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let text = all.join("\n");
        assert_eq!(
            text.matches("Read 1 file, ran 1 command").count(),
            8,
            "each face folds the run's steps to one line"
        );
        assert_eq!(text.matches("Follow up…").count(), 4);
        // The inline card has no anchor label: it is under its lines.
        assert_eq!(
            text.matches("src/manifest.rs:41").count(),
            1,
            "Conversation's card names the lines; the inline one does not"
        );
    }

    fn key_a() -> floem::event::Event {
        floem::event::Event::Key(floem::prelude::KeyboardEvent {
            state: floem::prelude::KeyState::Down,
            key: floem::prelude::Key::Character("a".into()),
            code: floem::prelude::Code::KeyA,
            location: Default::default(),
            modifiers: floem::prelude::Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    /// `a` on Conversation opens the composer in place, about the item as
    /// a whole, and leaves its box registered for the next `a` to focus.
    #[test]
    fn a_opens_the_in_place_composer_and_leaves_its_box_for_the_next_press() {
        let (mut harness, _, state) = app_harness();
        // A bare letter goes to the focused view, and a fresh window has
        // none; a click on the page is what a reader would have done.
        harness.click(700.0, 500.0);
        harness.rebuild();
        // A selection left over from Files does not anchor it.
        state.selection.set(Some((3, 5)));
        harness.dispatch_event(key_a());
        harness.rebuild();
        harness.paint();
        let composer = state.composer.get().expect("`a` did not open the composer");
        assert_eq!((composer.thread_id, composer.anchor), (None, None));
        assert!(state.composer_box.get().is_some());
    }

    /// The poll's status, which the main pane's key reads, must not build
    /// the page again: the reader's place on it is in the scroll, and a
    /// new scroll starts at the top.
    #[test]
    fn a_poll_leaves_the_page_where_it_was_scrolled_to() {
        use crate::backend::{Event, Status};

        let (mut harness, app, _) = app_harness();
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let viewport = scroll.get_visual_rect_no_clip();
        let title = find(harness.root_id(), "conversation-title").unwrap();
        let top = title.get_visual_rect_no_clip().y0;
        harness.scroll_down(viewport.center().x, viewport.center().y, 300.0);
        harness.rebuild();
        harness.paint();
        let scrolled = title.get_visual_rect_no_clip().y0;
        assert!(
            scrolled < top,
            "the page did not scroll: {top} then {scrolled}"
        );

        app.apply(Event::Status(Status::Refreshing));
        app.apply(Event::Status(Status::Idle {
            at: at("2026-09-18T10:00:00Z"),
            source: "GITHUB_TOKEN".into(),
        }));
        harness.rebuild();
        harness.paint();

        let after = find(harness.root_id(), "conversation").unwrap();
        assert_eq!(after, scroll, "the poll built the page again");
        assert_eq!(
            title.get_visual_rect_no_clip().y0,
            scrolled,
            "the poll moved the page"
        );
    }

    /// What a notification has the poll fetch lands on the page as it
    /// is: a comment said while the reader is on it is one more card in
    /// the same scroll, and the reader is where they were.
    #[test]
    fn a_comment_that_arrives_joins_the_page_in_place() {
        use crate::backend::Event;

        let (mut harness, app, state) = app_harness();
        let scroll = find(harness.root_id(), "conversation").unwrap();
        let viewport = scroll.get_visual_rect_no_clip();
        let title = find(harness.root_id(), "conversation-title").unwrap();
        harness.scroll_down(viewport.center().x, viewport.center().y, 300.0);
        harness.rebuild();
        harness.paint();
        let scrolled = title.get_visual_rect_no_clip().y0;
        let before = cards_in_order(&harness);

        let mut discussion = discussion();
        discussion.comments.push(IssueComment {
            id: 2,
            node_id: String::new(),
            user: "rohan".into(),
            user_avatar_url: String::new(),
            body: "One more thing.".into(),
            created_at: at("2026-09-18T09:00:00Z"),
            updated_at: None,
            html_url: "https://github.com/criccomini/perch/pull/57#issuecomment-2".into(),
            reactions: Default::default(),
        });
        app.apply_with_review(
            Event::DiscussionReady {
                item_id: "wt".into(),
                discussion: Arc::new(discussion),
            },
            state,
        );
        harness.rebuild();
        harness.paint();

        let mut expected = before.clone();
        expected.push("card:comment".into());
        assert_eq!(
            cards_in_order(&harness),
            expected,
            "the comment is not a new card"
        );
        assert_eq!(
            find(harness.root_id(), "conversation").unwrap(),
            scroll,
            "the comment built the page again"
        );
        assert_eq!(
            title.get_visual_rect_no_clip().y0,
            scrolled,
            "the comment moved the page"
        );
    }
}
