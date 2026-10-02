//! Cmd-F on Conversation: finding text on the page.
//!
//! The bar is the code's, from [`find`], hung from the top right of the
//! page with the same query and toggles. What it looks through is the
//! page's prose: every linked text view shown on it, which is every body,
//! a description's, a comment's, a review's and an agent's reply, with
//! the code fenced in them. Names, times, titles and the lines of events
//! are labels, and are not looked through.
//!
//! A match is a view and a byte range of its text, found line by line as
//! the code's are, and the views are taken in the order they read. Each
//! view tints its own matches, and the page scrolls to the one the bar
//! walks to. As a browser's bar does, a query typed on stays on the match
//! it is on while that still holds, and otherwise goes to the next; with
//! none to go on from, the first at or below the top of the page as it
//! is scrolled.
//!
//! The page's views come and go as it changes, with a reply streaming in
//! or a thread opened: after each layout, while the bar is up, the views
//! shown are checked against those last looked through, and the matches
//! made again when they differ, the bar staying on its match if it is
//! still there.

use std::cell::RefCell;
use std::rc::Rc;

use floem::kurbo::Rect;
use floem::reactive::{Effect, SignalGet, SignalUpdate, SignalWith};
use floem::ViewId;

use crate::review::find::{self, Match};
use crate::review::linked_text;
use crate::review::model::ReviewState;

/// One match: the linked text view it is in, its place among the views
/// looked through, and its byte range in the view's text, the end
/// exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub view: ViewId,
    pub order: usize,
    pub start: usize,
    pub end: usize,
}

impl Hit {
    fn same(&self, other: &Hit) -> bool {
        (self.view, self.start, self.end) == (other.view, other.start, other.end)
    }
}

/// How far past a match the page scrolls, above and below, so it is not
/// left against an edge or under the bar.
const MARGIN: f64 = 64.0;

thread_local! {
    /// The views last looked through, which carry the tints.
    static LOOKED: RefCell<Vec<ViewId>> = const { RefCell::new(Vec::new()) };
}

/// Why the matches are made again.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Why {
    /// The query or how it is read changed while the bar was up: go to
    /// the match to go on to, and scroll to it.
    Typed,
    /// The bar opened or closed, or the page changed under it: keep the
    /// match the bar is on if it is still there, and leave the page be.
    Changed,
}

/// Every match of `query` in the text of `views`, in order.
fn hits(views: &[ViewId], query: &str, options: find::Options) -> Result<Vec<Hit>, String> {
    let Some(regex) = find::pattern(query, options)? else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for (order, &view) in views.iter().enumerate() {
        linked_text::with_text(view, |text| {
            let mut at = 0;
            for line in text.split('\n') {
                for (start, end) in find::line_matches(&regex, line, options.whole_word) {
                    found.push(Hit {
                        view,
                        order,
                        start: at + start,
                        end: at + end,
                    });
                }
                at += line.len() + 1;
            }
        });
    }
    Ok(found)
}

/// The hits as the code's matches, with the view's place for the line,
/// for [`find::nearest`].
fn keyed(hits: &[Hit]) -> Vec<Match> {
    hits.iter()
        .map(|h| Match {
            display: h.order,
            start: h.start,
            end: h.end,
        })
        .collect()
}

/// Where a match is drawn in the window, once it is laid out.
fn window_rect(hit: &Hit) -> Option<Rect> {
    let local = linked_text::range_rect(hit.view, hit.start..hit.end)?;
    Some(hit.view.get_visual_transform().transform_rect_bbox(local))
}

/// The place to look from with no match to go on from: the first match
/// at or below the top of the page as it is scrolled, or past the end
/// when none is.
fn from_top(state: ReviewState, hits: &[Hit]) -> (usize, usize) {
    let top = state
        .conversation_page
        .get_untracked()
        .map(|page| page.get_visual_rect().y0);
    top.and_then(|top| {
        hits.iter()
            .find(|h| window_rect(h).is_some_and(|r| r.y0 >= top))
            .map(|h| (h.order, h.start))
    })
    .unwrap_or((usize::MAX, 0))
}

/// Each view looked through tinted with its matches, the one the bar is
/// on among them, and the views no longer looked through cleared.
fn tint(views: &[ViewId], hits: &[Hit], current: Option<usize>) {
    LOOKED.with_borrow(|before| {
        for view in before.iter().filter(|v| !views.contains(v)) {
            linked_text::mark_found(*view, Vec::new(), None);
        }
    });
    for &view in views {
        let mut ranges = Vec::new();
        let mut on = None;
        for (i, hit) in hits.iter().enumerate().filter(|(_, h)| h.view == view) {
            if Some(i) == current {
                on = Some(ranges.len());
            }
            ranges.push(hit.start..hit.end);
        }
        linked_text::mark_found(view, ranges, on);
    }
}

/// Scroll the page to a match.
fn show(hit: &Hit) {
    if let Some(rect) = linked_text::range_rect(hit.view, hit.start..hit.end) {
        hit.view.scroll_to(Some(rect.inflate(0.0, MARGIN)));
    }
}

/// Make the matches again, see [`Why`].
fn refresh(state: ReviewState, why: Why) {
    let open = state.conversation_find_open.get_untracked();
    let views = match state.conversation_page.get_untracked() {
        Some(page) if open => linked_text::shown_under(page),
        _ => Vec::new(),
    };
    let (hits, error) = if open {
        let query = state.find_query.get_untracked();
        match hits(&views, &query, state.find_options.get_untracked()) {
            Ok(hits) => (hits, None),
            Err(error) => (Vec::new(), Some(error)),
        }
    } else {
        (Vec::new(), None)
    };

    let before = state
        .conversation_find_current
        .get_untracked()
        .and_then(|i| {
            state
                .conversation_find_hits
                .with_untracked(|h| h.get(i).copied())
        });
    // The match the bar was on, where it is now, if it is still shown.
    let place = before.and_then(|b| {
        let order = views.iter().position(|v| *v == b.view)?;
        Some((order, b.start))
    });
    let current = match why {
        Why::Changed => before.and_then(|b| hits.iter().position(|h| h.same(&b))),
        Why::Typed => {
            let from = place.unwrap_or_else(|| from_top(state, &hits));
            find::nearest(&keyed(&hits), from, true)
        }
    };

    tint(&views, &hits, current);
    LOOKED.set(views);
    if state
        .conversation_find_error
        .with_untracked(|e| *e != error)
    {
        state.conversation_find_error.set(error);
    }
    let hits = Rc::new(hits);
    if state.conversation_find_hits.with_untracked(|h| *h != hits) {
        state.conversation_find_hits.set(hits.clone());
    }
    if state.conversation_find_current.get_untracked() != current {
        state.conversation_find_current.set(current);
    }
    if why == Why::Typed {
        if let Some(i) = current {
            show(&hits[i]);
        }
    }
}

/// Keeps the matches to the query: made again when it changes, or how it
/// is read, or the bar opens or closes. One effect for the page, made
/// with it.
pub fn wire(state: ReviewState) {
    Effect::new(move |before: Option<(bool, String, find::Options)>| {
        let now = (
            state.conversation_find_open.get(),
            state.find_query.get(),
            state.find_options.get(),
        );
        let typed = before
            .as_ref()
            .is_some_and(|b| b.0 && now.0 && (b.1 != now.1 || b.2 != now.2));
        refresh(state, if typed { Why::Typed } else { Why::Changed });
        now
    });
}

/// After a layout of the page: the matches made again if the views shown
/// are not the ones last looked through.
pub fn laid_out(state: ReviewState) {
    if !state.conversation_find_open.get_untracked() {
        return;
    }
    let Some(page) = state.conversation_page.get_untracked() else {
        return;
    };
    let shown = linked_text::shown_under(page);
    if LOOKED.with_borrow(|looked| *looked != shown) {
        refresh(state, Why::Changed);
    }
}

/// Enter, Shift-Enter and the arrows: the next match or the one before,
/// wrapping at either end, and the page scrolled to it. With none to go
/// on from, the first at or below the top of the page, or the last above
/// it.
pub fn walk(state: ReviewState, forward: bool) {
    let hits = state.conversation_find_hits.get_untracked();
    let n = hits.len();
    if n == 0 {
        return;
    }
    let index = match state.conversation_find_current.get_untracked() {
        Some(i) if forward => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        None => {
            let from = from_top(state, &hits);
            let Some(i) = find::nearest(&keyed(&hits), from, forward) else {
                return;
            };
            i
        }
    };
    let views = LOOKED.with_borrow(|v| v.clone());
    tint(&views, &hits, Some(index));
    state.conversation_find_current.set(Some(index));
    show(&hits[index]);
}

/// Escape, or Close: the bar away, its tints with it, and the keyboard
/// back to the page.
pub fn close(state: ReviewState) {
    let text_box = state.find_box.get_untracked();
    state.conversation_find_open.set(false);
    // The box goes with the bar and is told nothing, as over the code.
    state.find_focused.set(false);
    match state.conversation_page.get_untracked() {
        Some(page) => page.request_focus(),
        None => {
            if let Some(id) = text_box {
                id.clear_focus();
            }
        }
    }
}

/// The bar over the page in a headless window.
#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::prelude::*;
    use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
    use perch_github::{Discussion, IssueComment, Item, ItemSubject, PullRequest};

    use super::Hit;
    use crate::review::linked_text;
    use crate::review::model::{ItemTab, Phase, ReviewState};
    use crate::state::AppState;

    fn at(s: &str) -> jiff::Timestamp {
        s.parse().unwrap()
    }

    fn pull() -> PullRequest {
        PullRequest {
            number: 57,
            title: "Fence the epoch".into(),
            author_login: "rohan".into(),
            author_avatar_url: String::new(),
            head_sha: "h".into(),
            head_ref: "fence".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
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
            merged_at: None,
            merged_by: None,
            closed_at: None,
            requested_reviewers: vec![],
            milestone: None,
            labels: vec![],
            assignees: vec![],
        }
    }

    fn comment(id: u64, when: &str, body: &str) -> IssueComment {
        IssueComment {
            node_id: String::new(),
            reactions: Default::default(),
            id,
            user: "chris".into(),
            user_avatar_url: String::new(),
            body: body.into(),
            created_at: at(when),
            updated_at: None,
            html_url: format!("https://github.com/criccomini/perch/pull/57#issuecomment-{id}"),
        }
    }

    /// The description and a comment each name the epoch, three long
    /// comments push the page past the window, and the last comment
    /// names it again, twice, in a fenced block.
    fn discussion() -> Discussion {
        let filler = "A sentence that runs on well past the column's width. ".repeat(30);
        let mut comments = vec![comment(
            1,
            "2026-09-15T10:00:00Z",
            "The Epoch is read twice.",
        )];
        for i in 0..3 {
            comments.push(comment(10 + i, "2026-09-15T11:00:00Z", &filler));
        }
        comments.push(comment(
            20,
            "2026-09-16T10:00:00Z",
            "Bump it:\n\n```\nepoch += 1;\nsave(epoch);\n```",
        ));
        Discussion {
            node_id: String::new(),
            reactions: Default::default(),
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "Fence the manifest before the epoch check.".into(),
            comments,
            reviews: Vec::new(),
            review_comments: Vec::new(),
            events: Vec::new(),
            review_threads: Vec::new(),
        }
    }

    fn harness_with(discussion: Discussion) -> (HeadlessHarness, AppState, ReviewState) {
        let root = TestRoot::new();
        let app = AppState::new();
        let state = ReviewState::new();
        let pr = pull();
        let mut item = Item::new("criccomini/perch", at("2026-09-18T09:59:00Z"));
        item.id = "wt".into();
        item.subject = Some(ItemSubject::PullRequest(pr.clone()));
        state.item.set(Some(Arc::new(item)));
        state.ready.set(true);
        state.set_phase(Phase::Ready);
        state.pull_request.set(Some(Arc::new(pr)));
        state.discussion.set(Some(Arc::new(discussion)));
        state.composer_autofocus.set(false);
        let backend = crate::backend::Backend::detached();
        let view = crate::ui::app_view(app, state, Rc::new(backend));
        let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
        harness.rebuild();
        harness.paint();
        (harness, app, state)
    }

    /// Conversation under its find bar, all of it dark once the system is.
    #[test]
    fn goes_dark_with_the_system() {
        let mut harness = harness().0;
        let left = crate::theme::dark_check::light_left(&mut harness);
        assert!(left.is_empty(), "light colours left in dark:\n{}", left.join("\n"));
    }

    fn harness() -> (HeadlessHarness, AppState, ReviewState) {
        harness_with(discussion())
    }

    fn named(id: floem::ViewId, name: &str) -> Option<floem::ViewId> {
        if id.debug_name().split_whitespace().any(|part| part == name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn key(code: Code, key: Key, modifiers: Modifiers) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers,
            repeat: false,
            is_composing: false,
        })
    }

    fn cmd_f(harness: &mut HeadlessHarness) {
        harness.dispatch_event(key(
            Code::KeyF,
            Key::Character("f".into()),
            crate::ui::OS_MOD,
        ));
        harness.rebuild();
        harness.paint();
    }

    fn typed(harness: &mut HeadlessHarness, text: &str) {
        for ch in text.chars() {
            harness.dispatch_event(key(
                Code::Unidentified,
                Key::Character(ch.to_string()),
                Modifiers::empty(),
            ));
        }
        harness.rebuild();
        harness.paint();
    }

    fn press(harness: &mut HeadlessHarness, code: Code, named: NamedKey, modifiers: Modifiers) {
        harness.dispatch_event(key(code, Key::Named(named), modifiers));
        harness.rebuild();
        harness.paint();
    }

    /// The count's words, from the label's name, which quotes its text.
    fn count(harness: &HeadlessHarness) -> String {
        let id = named(harness.root_id(), "find-count").expect("the bar is up");
        let name = id.debug_name();
        let open = name.find('"').expect("the label quotes its text");
        let close = name.rfind('"').unwrap();
        name[open + 1..close].to_string()
    }

    fn found_text(hit: &Hit) -> String {
        linked_text::with_text(hit.view, |t| t[hit.start..hit.end].to_string()).unwrap()
    }

    /// Whether a match is drawn inside the page's scroll.
    fn in_view(state: ReviewState, hit: &Hit) -> bool {
        let page = state
            .conversation_page
            .get()
            .unwrap()
            .get_visual_rect_no_clip();
        super::window_rect(hit).is_some_and(|r| page.contains_rect(r))
    }

    #[test]
    fn cmd_f_on_the_page_finds_its_prose_in_order_and_escape_puts_the_bar_away() {
        let (mut harness, _, state) = harness();
        assert_eq!(state.tab.get(), ItemTab::Conversation);
        assert!(named(harness.root_id(), "find-bar").is_none());

        cmd_f(&mut harness);
        assert!(state.conversation_find_open.get());
        assert!(!state.find_open.get(), "the code's bar stays down");
        let text_box = named(harness.root_id(), "find-box").expect("the bar is up");
        assert!(harness.is_focused(text_box), "the box took the keyboard");

        typed(&mut harness, "epoch");
        let hits = state.conversation_find_hits.get();
        let words: Vec<_> = hits.iter().map(found_text).collect();
        assert_eq!(
            words,
            vec!["epoch", "Epoch", "epoch", "epoch"],
            "the description's, the comment's without regard to case, and the fenced \
             block's twice; not the title, which is a label"
        );
        assert!(
            hits.windows(2).all(|w| w[0].order <= w[1].order),
            "in the order the page reads"
        );
        assert_eq!(hits[2].view, hits[3].view, "one block, two lines");
        assert_eq!(state.conversation_find_current.get(), Some(0));
        assert_eq!(count(&harness), "1 of 4");
        assert!(state.composer.get().is_none(), "the box kept its letters");

        // The view the first match is in tints it as the current one.
        let (ranges, on) = linked_text::found_in(hits[0].view);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], hits[0].start..hits[0].end);
        assert_eq!(on, Some(0));
        let (ranges, on) = linked_text::found_in(hits[2].view);
        assert_eq!(ranges.len(), 2);
        assert_eq!(on, None);

        // Escape: the bar away with its tints, and the page has the
        // keyboard.
        press(
            &mut harness,
            Code::Escape,
            NamedKey::Escape,
            Modifiers::empty(),
        );
        assert!(!state.conversation_find_open.get());
        assert!(named(harness.root_id(), "find-bar").is_none());
        assert!(state.conversation_find_hits.with(|h| h.is_empty()));
        assert_eq!(linked_text::found_in(hits[0].view), (vec![], None));
        let page = state.conversation_page.get().unwrap();
        assert!(harness.is_focused(page), "the page has the keyboard back");
        assert_eq!(state.find_query.get(), "epoch", "kept for next time");
    }

    #[test]
    fn enter_walks_the_matches_scrolling_to_each_and_wrapping() {
        let (mut harness, _, state) = harness();
        cmd_f(&mut harness);
        typed(&mut harness, "epoch");
        let hits = state.conversation_find_hits.get();
        assert!(in_view(state, &hits[0]));
        assert!(!in_view(state, &hits[2]), "the block starts below the fold");

        press(
            &mut harness,
            Code::Enter,
            NamedKey::Enter,
            Modifiers::empty(),
        );
        assert_eq!(state.conversation_find_current.get(), Some(1));
        press(
            &mut harness,
            Code::Enter,
            NamedKey::Enter,
            Modifiers::empty(),
        );
        assert_eq!(state.conversation_find_current.get(), Some(2));
        assert!(in_view(state, &hits[2]), "the page scrolled to it");
        assert_eq!(count(&harness), "3 of 4");
        assert_eq!(linked_text::found_in(hits[2].view).1, Some(0));
        assert_eq!(linked_text::found_in(hits[0].view).1, None, "moved off");

        press(
            &mut harness,
            Code::Enter,
            NamedKey::Enter,
            Modifiers::empty(),
        );
        press(
            &mut harness,
            Code::Enter,
            NamedKey::Enter,
            Modifiers::empty(),
        );
        assert_eq!(
            state.conversation_find_current.get(),
            Some(0),
            "past the last is the first"
        );
        assert!(in_view(state, &hits[0]), "and the page went back up");

        press(&mut harness, Code::Enter, NamedKey::Enter, Modifiers::SHIFT);
        assert_eq!(
            state.conversation_find_current.get(),
            Some(3),
            "before the first is the last"
        );

        // Cmd-G walks too, and the arrows.
        harness.dispatch_event(key(
            Code::KeyG,
            Key::Character("g".into()),
            crate::ui::OS_MOD,
        ));
        harness.rebuild();
        assert_eq!(state.conversation_find_current.get(), Some(0));
        let next = named(harness.root_id(), "find-next").unwrap();
        let center = next.get_visual_rect_no_clip().center();
        harness.click(center.x, center.y);
        harness.rebuild();
        assert_eq!(state.conversation_find_current.get(), Some(1));
        assert!(state.find_focused.get(), "the box keeps the keyboard");
    }

    #[test]
    fn a_query_typed_on_stays_on_its_match_while_it_holds() {
        let (mut harness, _, state) = harness();
        cmd_f(&mut harness);
        typed(&mut harness, "ep");
        press(
            &mut harness,
            Code::Enter,
            NamedKey::Enter,
            Modifiers::empty(),
        );
        let on = |state: ReviewState| {
            let i = state.conversation_find_current.get().unwrap();
            state.conversation_find_hits.with(|h| h[i])
        };
        let comment = on(state);
        assert_eq!(found_text(&comment), "Ep", "the comment's");

        // `epoch` still holds there: the bar stays, rather than going
        // back to the top.
        typed(&mut harness, "och");
        let now = on(state);
        assert_eq!((now.view, now.start), (comment.view, comment.start));
        assert_eq!(count(&harness), "2 of 4");

        // With case minded it does not, and the bar goes on to the next
        // that does, the fenced block's.
        harness.dispatch_event(key(
            Code::KeyC,
            Key::Character("c".into()),
            crate::ui::OS_MOD | Modifiers::ALT,
        ));
        harness.rebuild();
        assert!(state.find_options.get().match_case);
        let next = on(state);
        assert_eq!(found_text(&next), "epoch");
        assert!(next.order > comment.order);
        assert_eq!(count(&harness), "2 of 3");
    }

    #[test]
    fn a_comment_that_arrives_while_the_bar_is_up_is_looked_through() {
        let (mut harness, app, state) = harness();
        cmd_f(&mut harness);
        typed(&mut harness, "epoch");
        assert_eq!(state.conversation_find_hits.with(|h| h.len()), 4);
        let on = state.conversation_find_hits.with(|h| h[0]);

        let mut more = discussion();
        more.comments
            .push(comment(30, "2026-09-17T10:00:00Z", "One more epoch."));
        app.apply_with_review(
            crate::backend::Event::DiscussionReady {
                item_id: "wt".into(),
                discussion: Arc::new(more),
            },
            state,
        );
        harness.rebuild();
        harness.paint();
        assert_eq!(state.conversation_find_hits.with(|h| h.len()), 5);
        assert_eq!(count(&harness), "1 of 5");
        let current = state.conversation_find_current.get().unwrap();
        assert!(
            state.conversation_find_hits.with(|h| h[current].same(&on)),
            "the bar stayed on its match"
        );
    }

    #[test]
    fn each_tab_has_its_own_bar() {
        let (mut harness, _, state) = harness();
        state.item.update(|item| {
            let mut with = (**item.as_ref().unwrap()).clone();
            with.checkout = Some(perch_github::ItemCheckout {
                branch: "perch/pr-57".into(),
                base_ref: "main".into(),
                head_sha: "h".into(),
                path: std::path::PathBuf::from("/tmp/wt"),
                owned: true,
                on_disk: true,
                churn: None,
                last_seen_head: None,
                divergence: None,
                source: None,
            });
            *item = Some(Arc::new(with));
        });
        state.open_tab("src/x.rs");
        state.finish_load(
            "src/x.rs",
            Arc::new(crate::backend::build_loaded_file(
                "src/x.rs".into(),
                "let epoch = 1;\n".into(),
                "let epoch = 0;\n".into(),
                Some(perch_git::ChangeStatus::Modified),
                false,
                perch_git::Scope::Branch,
                String::new(),
            )),
        );
        state.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();

        cmd_f(&mut harness);
        typed(&mut harness, "epoch");
        assert_eq!(state.conversation_find_hits.with(|h| h.len()), 4);

        // The code's bar is down: the page's being up is nothing to it.
        state.show_tab(ItemTab::Code);
        harness.rebuild();
        harness.paint();
        assert!(state.conversation_page.get().is_none(), "the page is gone");
        assert!(!state.find_open.get());
        assert!(named(harness.root_id(), "find-bar").is_none());

        // Back on Conversation, the page's bar is up on a new page, with
        // its matches and none walked to yet.
        state.show_tab(ItemTab::Conversation);
        harness.rebuild();
        harness.paint();
        assert!(named(harness.root_id(), "find-bar").is_some(), "still up");
        assert_eq!(state.conversation_find_hits.with(|h| h.len()), 4);
        assert_eq!(count(&harness), "? of 4");
    }
}
