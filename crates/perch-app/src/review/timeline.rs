//! The Conversation tab's cards, in order. Pure: a pull request, its
//! discussion, an issue and its comments, and the local threads go in, and
//! one sorted list of cards comes out. This is where the interleave lives,
//! so it can be checked without a window.
//!
//! GitHub's three lists do not nest. A review's line comments name their
//! review by id, a reply names the comment it answers, and a reply's own
//! `pull_request_review_id` is a review of its own that GitHub made for it,
//! with no body. The nesting is rebuilt here, and that empty review is the
//! noise that gets dropped.

use jiff::Timestamp;
use perch_github::{CommentThread, Discussion, Issue, IssueComment, PullEvent, PullEventKind, PullRequest, Review, ReviewComment};

/// One card on Conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum Card {
    /// The pull request's or issue's own text, led by who opened it. Always
    /// first.
    Description { author: String, avatar_url: String, at: Option<Timestamp>, body: String },
    Comment(IssueComment),
    /// A review with its verdict and the line comments that belong to it.
    /// `review` is None for line comments whose review is not in the list
    /// (GitHub sends those for single comments made outside a review); such
    /// a card reads as "commented".
    Review { review: Option<Review>, comments: Vec<LineThread> },
    Event(PullEvent),
    Thread(CommentThread),
}

/// A line comment and the replies under it, in time order.
#[derive(Debug, Clone, PartialEq)]
pub struct LineThread {
    pub comment: ReviewComment,
    pub replies: Vec<ReviewComment>,
}

/// How many characters of a first line the harness shows.
const FIRST_LINE_CHARS: usize = 80;

impl Card {
    /// The time the card sorts by: a thread by its first message, a review
    /// by when it was submitted or else by its first comment. `None` sorts
    /// first, and the description is first whatever its time.
    pub fn at(&self) -> Option<Timestamp> {
        match self {
            Card::Description { at, .. } => *at,
            Card::Comment(c) => Some(c.created_at),
            Card::Review { review, comments } => review
                .as_ref()
                .and_then(|r| r.submitted_at)
                .or_else(|| comments.first().map(|c| c.comment.created_at)),
            Card::Event(e) => Some(e.at),
            Card::Thread(t) => Some(t.messages.first().map(|m| m.created_at).unwrap_or(t.created_at)),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Card::Description { .. } => "description",
            Card::Comment(_) => "comment",
            Card::Review { .. } => "review",
            Card::Event(_) => "event",
            Card::Thread(_) => "thread",
        }
    }

    /// For the harness: the first non-empty line of what the card says,
    /// cut to a terminal's width. A review leads with its verdict, since
    /// its body is often empty.
    pub fn first_line(&self) -> String {
        match self {
            Card::Description { body, .. } if body.trim().is_empty() => "No description.".into(),
            Card::Description { body, .. } => first_line(body),
            Card::Comment(c) => first_line(&c.body),
            Card::Review { review, .. } => {
                let verdict = verdict(review.as_ref());
                match review.as_ref().map(|r| first_line(&r.body)).filter(|l| !l.is_empty()) {
                    Some(summary) => cut(&format!("{verdict}  {summary}")),
                    None => verdict.into(),
                }
            }
            Card::Event(e) => event_name(&e.kind),
            Card::Thread(t) => t.messages.first().map(|m| first_line(&m.body)).unwrap_or_default(),
        }
    }

    /// Where two cards share a time: what the pull request did, then what
    /// people said about it, then what the reader asked.
    fn rank(&self) -> u8 {
        match self {
            Card::Description { .. } => 0,
            Card::Event(_) => 1,
            Card::Comment(_) => 2,
            Card::Review { .. } => 3,
            Card::Thread(_) => 4,
        }
    }
}

/// The word in a review's byline. Anything GitHub sends that is not a
/// verdict, and a card with no review, is "commented".
pub fn verdict(review: Option<&Review>) -> &'static str {
    match review.map(|r| r.state.as_str()) {
        Some("APPROVED") => "approved",
        Some("CHANGES_REQUESTED") => "requested changes",
        Some("DISMISSED") => "dismissed",
        _ => "commented",
    }
}

/// The event's one line.
pub fn event_name(kind: &PullEventKind) -> String {
    match kind {
        PullEventKind::Opened => "opened".into(),
        PullEventKind::Merged { by: Some(by) } => format!("merged by {by}"),
        PullEventKind::Merged { by: None } => "merged".into(),
        PullEventKind::Closed => "closed".into(),
        PullEventKind::Reopened => "reopened".into(),
        PullEventKind::Draft => "converted to draft".into(),
        PullEventKind::Ready => "ready for review".into(),
    }
}

/// The first non-empty line, trimmed and cut to the harness's width.
pub fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(cut).unwrap_or_default()
}

fn cut(line: &str) -> String {
    line.chars().take(FIRST_LINE_CHARS).collect()
}

/// The cards for one item, sorted. A pull request's description comes
/// from the discussion when there is one, else from the pull request alone
/// with no body; an issue's from the issue. A bare branch has neither and
/// its timeline is its threads.
pub fn timeline(
    pull: Option<&PullRequest>,
    discussion: Option<&Discussion>,
    issue: Option<(&Issue, &[IssueComment])>,
    threads: &[CommentThread],
) -> Vec<Card> {
    let mut cards = Vec::new();

    match (pull, issue) {
        (Some(pr), _) => cards.push(Card::Description {
            author: pr.author_login.clone(),
            avatar_url: pr.author_avatar_url.clone(),
            at: discussion.and_then(|d| d.opened_at),
            body: discussion.map(|d| d.body.clone()).unwrap_or_default(),
        }),
        (None, Some((issue, _))) => cards.push(Card::Description {
            author: issue.author_login.clone(),
            avatar_url: issue.author_avatar_url.clone(),
            at: issue.created_at,
            body: issue.body.clone(),
        }),
        (None, None) => {}
    }

    if let Some(d) = discussion {
        cards.extend(d.comments.iter().cloned().map(Card::Comment));
        cards.extend(d.events.iter().cloned().map(Card::Event));
        cards.extend(review_cards(&d.reviews, &d.review_comments));
    }
    if let Some((_, comments)) = issue {
        cards.extend(comments.iter().cloned().map(Card::Comment));
    }
    cards.extend(threads.iter().cloned().map(Card::Thread));

    // The sort is stable, so cards a list already had in order keep it.
    cards.sort_by_key(|c| (!matches!(c, Card::Description { .. }), c.at(), c.rank()));
    cards
}

/// The review cards: each review with the line threads under it, and a
/// card with no review for each review id the list does not carry.
fn review_cards(reviews: &[Review], comments: &[ReviewComment]) -> Vec<Card> {
    let mut threads = line_threads(comments);
    let mut cards = Vec::new();

    for review in reviews {
        // A pending review is the reader's own draft, and so are its
        // comments; none of it has been said yet.
        if review.state == "PENDING" {
            threads.retain(|t| t.comment.pull_request_review_id != Some(review.id));
            continue;
        }
        let mine: Vec<LineThread> = threads
            .extract_if(.., |t| t.comment.pull_request_review_id == Some(review.id))
            .collect();
        if review.state == "COMMENTED" && review.body.trim().is_empty() && mine.is_empty() {
            continue;
        }
        cards.push(Card::Review { review: Some(review.clone()), comments: mine });
    }

    // What is left names a review that was not sent. Comments of one such
    // review stay together, as they would have under it.
    while let Some(first) = threads.first() {
        let id = first.comment.pull_request_review_id;
        let mine: Vec<LineThread> = threads.extract_if(.., |t| t.comment.pull_request_review_id == id).collect();
        cards.push(Card::Review { review: None, comments: mine });
    }
    cards
}

/// Nest replies under the comment they answer, in time order. A reply
/// points at the first comment of its thread, but a chain is followed all
/// the same, and a reply whose parent is missing starts a thread of its own.
pub fn line_threads(comments: &[ReviewComment]) -> Vec<LineThread> {
    let root_of = |c: &ReviewComment| -> u64 {
        let mut at = c;
        // Bounded, in case the ids loop.
        for _ in 0..comments.len() {
            let Some(parent) = at.in_reply_to_id.and_then(|id| comments.iter().find(|p| p.id == id)) else {
                break;
            };
            at = parent;
        }
        at.id
    };

    let mut threads: Vec<LineThread> = comments
        .iter()
        .filter(|c| root_of(c) == c.id)
        .map(|c| LineThread { comment: c.clone(), replies: Vec::new() })
        .collect();
    for c in comments.iter().filter(|c| root_of(c) != c.id) {
        let root = root_of(c);
        if let Some(t) = threads.iter_mut().find(|t| t.comment.id == root) {
            t.replies.push(c.clone());
        }
    }
    for t in &mut threads {
        t.replies.sort_by_key(|r| r.created_at);
    }
    threads.sort_by_key(|t| t.comment.created_at);
    threads
}

/// The fixtures are shared with the harness's test.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use perch_github::{Anchor, Author, CommentMessage, Side, ThreadOrigin};

    pub(crate) fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn pull() -> PullRequest {
        PullRequest {
            number: 57,
            title: "Fence the manifest".into(),
            author_login: "rohan".into(),
            author_avatar_url: "https://avatars/2".into(),
            head_sha: "h".into(),
            head_ref: "fence".into(),
            base_sha: "b".into(),
            base_ref: "main".into(),
            draft: false,
            merged: false,
            state: "open".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
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

    fn issue() -> Issue {
        Issue {
            number: 42,
            title: "Fence the manifest".into(),
            body: "The manifest can be written twice.".into(),
            state: "open".into(),
            state_reason: None,
            author_login: "rohan".into(),
            author_avatar_url: "https://avatars/2".into(),
            labels: vec![],
            assignees: vec![],
            comments: 0,
            created_at: Some(at("2026-09-14T14:02:00Z")),
            updated_at: at("2026-09-16T11:40:00Z"),
            repo_full_name: "criccomini/perch".into(),
        }
    }

    fn discussion(reviews: Vec<Review>, review_comments: Vec<ReviewComment>) -> Discussion {
        Discussion {
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            comments: vec![],
            reviews,
            review_comments,
            events: vec![],
        }
    }

    fn comment(id: u64, when: &str, body: &str) -> IssueComment {
        IssueComment {
            id,
            user: "chris".into(),
            user_avatar_url: String::new(),
            body: body.into(),
            created_at: at(when),
            updated_at: None,
            html_url: format!("https://github.com/a/b/pull/57#issuecomment-{id}"),
        }
    }

    pub(crate) fn review(id: u64, state: &str, body: &str, when: Option<&str>) -> Review {
        Review {
            id,
            user: "chris".into(),
            body: body.into(),
            state: state.into(),
            submitted_at: when.map(at),
            html_url: String::new(),
        }
    }

    pub(crate) fn line(id: u64, review: Option<u64>, reply_to: Option<u64>, when: &str, body: &str) -> ReviewComment {
        ReviewComment {
            id,
            pull_request_review_id: review,
            in_reply_to_id: reply_to,
            user: "chris".into(),
            body: body.into(),
            path: "src/manifest.rs".into(),
            line: Some(41),
            original_line: None,
            diff_hunk: String::new(),
            side: Side::Right,
            created_at: at(when),
            updated_at: None,
            html_url: String::new(),
        }
    }

    pub(crate) fn event(kind: PullEventKind, when: &str) -> PullEvent {
        PullEvent { kind, at: at(when) }
    }

    /// A thread made at `created`, whose question was asked at `asked`.
    fn thread(id: &str, created: &str, asked: &str, question: &str) -> CommentThread {
        CommentThread {
            id: id.into(),
            worktree_id: "w".into(),
            origin: ThreadOrigin::Local,
            anchor: Some(Anchor { path: "src/x.rs".into(), start_line: 41, line: 58, side: Side::Right, commit_sha: "h".into() }),
            outdated: false,
            resolved: false,
            created_at: at(created),
            messages: vec![CommentMessage {
                id: format!("{id}-q"),
                thread_id: id.into(),
                origin: ThreadOrigin::Local,
                author: Author::User,
                body: question.into(),
                created_at: at(asked),
                run: None,
            }],
        }
    }

    fn kinds(cards: &[Card]) -> Vec<&'static str> {
        cards.iter().map(Card::kind).collect()
    }

    fn lines(cards: &[Card]) -> Vec<String> {
        cards.iter().map(Card::first_line).collect()
    }

    #[test]
    fn a_reviews_line_comments_stay_inside_it_whatever_their_time() {
        let mut d = discussion(
            vec![review(9, "APPROVED", "Looks right.", Some("2026-09-16T11:40:00Z"))],
            vec![
                line(31, Some(9), None, "2026-09-16T11:38:00Z", "epoch"),
                line(32, Some(9), None, "2026-09-16T12:10:00Z", "fence"),
            ],
        );
        d.comments.push(comment(1, "2026-09-16T12:00:00Z", "Thanks"));
        let cards = timeline(Some(&pull()), Some(&d), None, &[]);
        assert_eq!(kinds(&cards), ["description", "review", "comment"]);
        assert_eq!(lines(&cards), ["The description.", "approved  Looks right.", "Thanks"]);
        let Card::Review { review, comments } = &cards[1] else { panic!("{cards:?}") };
        assert_eq!(review.as_ref().map(|r| r.id), Some(9));
        assert_eq!(comments.iter().map(|t| t.comment.id).collect::<Vec<_>>(), [31, 32], "in time order");
        assert_eq!(cards[1].at(), Some(at("2026-09-16T11:40:00Z")), "the review's own time");
    }

    #[test]
    fn replies_nest_under_their_parent_and_orphans_become_top_level() {
        let d = discussion(
            vec![review(9, "COMMENTED", "", Some("2026-09-16T11:40:00Z"))],
            vec![
                line(31, Some(9), None, "2026-09-16T11:38:00Z", "epoch"),
                // A reply's own review is GitHub's empty one; it belongs
                // with its parent.
                line(33, Some(10), Some(32), "2026-09-16T13:00:00Z", "the chain"),
                line(32, Some(10), Some(31), "2026-09-16T12:00:00Z", "the reply"),
                line(34, Some(11), Some(999), "2026-09-16T12:30:00Z", "the orphan"),
            ],
        );
        let cards = timeline(Some(&pull()), Some(&d), None, &[]);
        assert_eq!(kinds(&cards), ["description", "review", "review"]);
        let Card::Review { review, comments } = &cards[1] else { panic!("{cards:?}") };
        assert_eq!(review.as_ref().map(|r| r.id), Some(9));
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].comment.id, 31);
        assert_eq!(comments[0].replies.iter().map(|r| r.id).collect::<Vec<_>>(), [32, 33]);
        let Card::Review { review, comments } = &cards[2] else { panic!("{cards:?}") };
        assert_eq!(*review, None, "the orphan's review 11 is not in the list");
        assert_eq!(comments[0].comment.id, 34);
        assert!(comments[0].replies.is_empty());
    }

    #[test]
    fn a_thread_sorts_by_its_question_between_two_comments() {
        let mut d = discussion(vec![], vec![]);
        d.comments.push(comment(1, "2026-09-16T10:00:00Z", "first"));
        d.comments.push(comment(2, "2026-09-16T12:00:00Z", "second"));
        let t = thread("t1", "2026-09-16T09:30:00Z", "2026-09-16T11:00:00Z", "why the fence?");
        let cards = timeline(Some(&pull()), Some(&d), None, &[t]);
        assert_eq!(kinds(&cards), ["description", "comment", "thread", "comment"]);
        assert_eq!(cards[2].first_line(), "why the fence?");
        assert_eq!(cards[2].at(), Some(at("2026-09-16T11:00:00Z")), "the question, not the row");
    }

    #[test]
    fn events_land_between_comments_in_time_order() {
        let mut d = discussion(vec![], vec![]);
        d.comments.push(comment(1, "2026-09-15T10:00:00Z", "first"));
        d.comments.push(comment(2, "2026-09-17T15:00:00Z", "at the same moment as the merge"));
        d.events = vec![
            event(PullEventKind::Opened, "2026-09-14T14:02:00Z"),
            event(PullEventKind::Merged { by: Some("rohan".into()) }, "2026-09-17T15:00:00Z"),
        ];
        let cards = timeline(Some(&pull()), Some(&d), None, &[]);
        assert_eq!(kinds(&cards), ["description", "event", "comment", "event", "comment"]);
        assert_eq!(lines(&cards)[1], "opened");
        assert_eq!(lines(&cards)[3], "merged by rohan", "an event wins a tie");
    }

    #[test]
    fn an_issue_has_a_description_comments_and_threads_and_no_events() {
        let issue = issue();
        let comments = vec![comment(1, "2026-09-15T10:00:00Z", "Seen it too.")];
        let t = thread("t1", "2026-09-16T09:00:00Z", "2026-09-16T09:00:00Z", "where?");
        let cards = timeline(None, None, Some((&issue, &comments)), &[t]);
        assert_eq!(kinds(&cards), ["description", "comment", "thread"]);
        let Card::Description { author, avatar_url, at: opened, body } = &cards[0] else { panic!("{cards:?}") };
        assert_eq!(author, "rohan");
        assert_eq!(avatar_url, "https://avatars/2");
        assert_eq!(*opened, Some(at("2026-09-14T14:02:00Z")));
        assert_eq!(body, "The manifest can be written twice.");
    }

    #[test]
    fn the_description_is_first_even_when_a_thread_predates_it() {
        let d = discussion(vec![], vec![]);
        let t = [thread("t1", "2026-09-01T09:00:00Z", "2026-09-01T09:00:00Z", "early")];
        let cards = timeline(Some(&pull()), Some(&d), None, &t);
        assert_eq!(kinds(&cards), ["description", "thread"]);

        // Without a discussion the description is the pull request alone.
        let cards = timeline(Some(&pull()), None, None, &t);
        assert_eq!(kinds(&cards), ["description", "thread"]);
        assert_eq!(cards[0].at(), None);
        assert_eq!(cards[0].first_line(), "No description.");

        // A bare branch has no description at all.
        let cards = timeline(None, None, None, &t);
        assert_eq!(kinds(&cards), ["thread"]);
    }

    #[test]
    fn pending_and_empty_commented_reviews_are_dropped() {
        let d = discussion(
            vec![
                review(1, "PENDING", "draft", None),
                review(2, "COMMENTED", "  ", Some("2026-09-16T11:00:00Z")),
                review(3, "COMMENTED", "", Some("2026-09-16T11:10:00Z")),
                review(4, "DISMISSED", "", Some("2026-09-16T11:20:00Z")),
            ],
            vec![
                // The pending review's draft comment goes with it.
                line(31, Some(1), None, "2026-09-16T10:00:00Z", "not yet said"),
                line(32, Some(3), None, "2026-09-16T11:09:00Z", "one line comment keeps the review"),
            ],
        );
        let cards = timeline(Some(&pull()), Some(&d), None, &[]);
        assert_eq!(kinds(&cards), ["description", "review", "review"]);
        assert_eq!(lines(&cards)[1..], ["commented", "dismissed"]);
        let Card::Review { review, .. } = &cards[1] else { panic!("{cards:?}") };
        assert_eq!(review.as_ref().map(|r| r.id), Some(3));
    }

    #[test]
    fn a_line_comment_with_no_review_becomes_a_review_card_with_review_none() {
        let d = discussion(vec![], vec![line(31, Some(77), None, "2026-09-16T11:38:00Z", "epoch")]);
        let cards = timeline(Some(&pull()), Some(&d), None, &[]);
        assert_eq!(kinds(&cards), ["description", "review"]);
        assert_eq!(cards[1].first_line(), "commented");
        assert_eq!(cards[1].at(), Some(at("2026-09-16T11:38:00Z")), "the first comment's time");
        let Card::Review { review, comments } = &cards[1] else { panic!("{cards:?}") };
        assert_eq!(*review, None);
        assert_eq!(comments[0].comment.id, 31);
    }

    #[test]
    fn a_review_without_a_time_sorts_by_its_first_comment() {
        let mut d = discussion(
            vec![review(9, "APPROVED", "", None)],
            vec![line(31, Some(9), None, "2026-09-16T11:38:00Z", "epoch")],
        );
        d.comments.push(comment(1, "2026-09-16T11:00:00Z", "before"));
        d.comments.push(comment(2, "2026-09-16T12:00:00Z", "after"));
        let cards = timeline(Some(&pull()), Some(&d), None, &[]);
        assert_eq!(lines(&cards), ["The description.", "before", "approved", "after"]);
    }

    #[test]
    fn first_lines_skip_blanks_and_are_cut_to_width() {
        let long = "x".repeat(100);
        let c = Card::Comment(comment(1, "2026-09-16T11:00:00Z", &format!("\n   \n  first  \nsecond\n{long}")));
        assert_eq!(c.first_line(), "first");
        let c = Card::Comment(comment(1, "2026-09-16T11:00:00Z", &long));
        assert_eq!(c.first_line().chars().count(), 80);
        let c = Card::Comment(comment(1, "2026-09-16T11:00:00Z", " \n\t"));
        assert_eq!(c.first_line(), "");
        assert_eq!(Card::Event(event(PullEventKind::Draft, "2026-09-16T11:00:00Z")).first_line(), "converted to draft");
        assert_eq!(verdict(Some(&review(1, "CHANGES_REQUESTED", "", None))), "requested changes");
        assert_eq!(verdict(Some(&review(1, "SOMETHING_NEW", "", None))), "commented");
    }
}
