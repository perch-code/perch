//! Conversation: the item's description, everything said about it on
//! GitHub, and every thread the reader has had with an agent about it, in
//! one quiet reading column in time order, with the composer at the end.
//!
//! The order is `review::timeline`'s; this module only draws it. The cards
//! are in `review::cards`, and a thread's card shares its views with the
//! thread column on Files.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalWith};
use floem::text::FontWeight;
use floem::views::{
    dyn_container, dyn_stack, Container, Decorators, Empty, Label, ScrollExt, Stack,
};

use crate::backend::Backend;
use crate::review::cards;
use crate::review::model::ReviewState;
use crate::review::threads::{self, ThreadFace};
use crate::review::timeline::{timeline, Card};
use crate::review::OpenFile;
use crate::state::AppState;
use crate::theme;

/// One scrolling column, centred, on the page grey.
pub fn view(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    let card_backend = backend.clone();
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
        let threads = state.comment_threads.get();
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
    };

    Container::new(
        Stack::vertical((
            conversation_title(state),
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
        .style(|s| {
            s.flex_col()
                .width_full()
                .max_width(theme::CONVERSATION_WIDTH)
        }),
    )
    .style(|s| {
        s.width_full()
            .items_start()
            .justify_center()
            .padding(theme::CONVERSATION_PADDING)
    })
    .scroll()
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
        // What Perch started from an issue is in the strip under the
        // header, as chips, not repeated under the description.
        Card::Description {
            author,
            avatar_url,
            at,
            body,
        } => cards::description_card(app, author, avatar_url, at, body).into_any(),
        Card::Comment(comment) => cards::comment_card(app, state, comment).into_any(),
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
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            comments: vec![IssueComment {
                id: 1,
                user: "chris".into(),
                user_avatar_url: String::new(),
                body: "Thanks".into(),
                created_at: at("2026-09-15T10:00:00Z"),
                updated_at: None,
                html_url: "https://github.com/criccomini/perch/pull/57#issuecomment-1".into(),
            }],
            reviews: vec![Review {
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
        state.discussion.set(Some(Arc::new(discussion())));
        state.comment_threads.set(Arc::new(vec![thread()]));
        // See the field: a focused editor cannot be driven headlessly.
        state.composer_autofocus.set(false);
        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
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

    /// The thread views build for both faces, from the same thread.
    #[test]
    fn a_thread_draws_with_either_face() {
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
                ThreadFace::Column,
            ),
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
                ThreadFace::Column,
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
            crate::review::threads::thread_card(
                app,
                state,
                backend,
                open_file,
                thread(),
                ThreadFace::Inline,
            ),
        ));
        let mut harness = HeadlessHarness::new_with_size(root, view, 800.0, 1600.0);
        harness.rebuild();
        harness.paint();
        let mut all = Vec::new();
        names(harness.root_id(), &mut all);
        let text = all.join("\n");
        assert_eq!(
            text.matches("Read 1 file, ran 1 command").count(),
            10,
            "each face folds the run's steps to one line"
        );
        assert_eq!(text.matches("Follow up…").count(), 5);
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
    /// That second press asks the window for IME, which the headless
    /// window cannot give, so it is not pressed here.
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
}
