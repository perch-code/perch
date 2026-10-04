//! The Conversation tab's cards, in order. Pure: a pull request, its
//! discussion, an issue and its comments, and the local threads go in, and
//! one sorted list of cards comes out. This is where the interleave lives,
//! so it can be checked without a window.
//!
//! A review's line comments are not here. The cache turns them into
//! comment threads, and a thread from GitHub is a `Card::Thread` like one of
//! Perch's own, ordered by its first comment. A reply's own
//! `pull_request_review_id` is a review of its own that GitHub made for it,
//! with no body, and that empty review is the noise that gets dropped.

use jiff::Timestamp;
use perch_git::Commit;
use perch_github::{
    CommentThread, Discussion, Issue, IssueComment, Label, PullEvent, PullEventKind, PullRequest,
    Reference, Review,
};

/// One card on Conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum Card {
    /// The pull request's or issue's own text. Always first; who opened it
    /// is the summary's Author row. `node_id` is what reactions on it are
    /// left by; empty when the pull request's discussion has not landed, or
    /// an older build kept it.
    Description { at: Option<Timestamp>, body: String, node_id: String, editable: bool },
    Comment(IssueComment),
    /// A review with its verdict and summary. Its line comments are threads
    /// of their own.
    Review { review: Review },
    Event(PullEvent),
    /// One of Perch's threads or one of GitHub's; `origin` tells them apart.
    Thread(CommentThread),
}

/// How many characters of a first line the harness shows.
const FIRST_LINE_CHARS: usize = 80;

impl Card {
    /// The time the card sorts by: a thread by its first message, a review
    /// by when it was submitted. `None` sorts first, and the description is
    /// first whatever its time.
    pub fn at(&self) -> Option<Timestamp> {
        match self {
            Card::Description { at, .. } => *at,
            Card::Comment(c) => Some(c.created_at),
            Card::Review { review } => review.submitted_at,
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
            Card::Review { review } => {
                let verdict = verdict(review);
                match Some(first_line(&review.body)).filter(|l| !l.is_empty()) {
                    Some(summary) => cut(&format!("{verdict}  {summary}")),
                    None => verdict.into(),
                }
            }
            Card::Event(e) => cut(&event_name(e)),
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
/// verdict is "commented".
pub fn verdict(review: &Review) -> &'static str {
    match review.state.as_str() {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "requested changes",
        "DISMISSED" => "dismissed",
        _ => "commented",
    }
}

/// A piece of an event's line. Every piece has words, which is what the
/// harness prints; the view draws a label in its own colours, a commit in
/// the code face, and a reference as somewhere to go.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    Label(Label),
    /// A commit, by the seven characters everyone reads it as, that may
    /// be in another repository: a reference to this one names it.
    Sha(String),
    /// A commit on the head branch, which opens: on Files by itself when
    /// the checkout has it, else on GitHub.
    Commit(String),
    /// A commit's subject, where a file it names is a link to it.
    Subject(String),
    Reference(Reference),
}

impl Piece {
    fn words(text: impl Into<String>) -> Self {
        Piece::Text(text.into())
    }

    /// What the piece says. A sha is cut to the length git shows, and a
    /// reference reads as it is drawn: where it points, then its title.
    pub fn text(&self) -> String {
        match self {
            Piece::Text(text) | Piece::Subject(text) => text.clone(),
            Piece::Label(label) => label.name.clone(),
            Piece::Sha(sha) | Piece::Commit(sha) => short_sha(sha),
            Piece::Reference(reference) => {
                format!("{} {}", reference.slug(), reference.title).trim_end().into()
            }
        }
    }
}

/// The seven characters a commit is read by.
pub fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// The event's one line, in the order its pieces are drawn. Who did it is
/// drawn before these and is not among them, so a line that names the
/// same person twice says "self" instead.
pub fn event_pieces(event: &PullEvent) -> Vec<Piece> {
    let same = |who: &str| !who.is_empty() && who == event.actor;
    match &event.kind {
        PullEventKind::Opened => vec![Piece::words("opened")],
        PullEventKind::Merged => vec![Piece::words("merged")],
        PullEventKind::Closed => vec![Piece::words("closed")],
        PullEventKind::Reopened => vec![Piece::words("reopened")],
        PullEventKind::Draft => vec![Piece::words("converted this to draft")],
        PullEventKind::Ready => vec![Piece::words("marked this ready for review")],
        PullEventKind::Committed { sha, subject, .. } => vec![
            Piece::words("committed"),
            Piece::Commit(sha.clone()),
            Piece::Subject(subject.clone()),
        ],
        PullEventKind::ForcePushed { sha } => {
            vec![Piece::words("force-pushed to"), Piece::Commit(sha.clone())]
        }
        PullEventKind::HeadDeleted => vec![Piece::words("deleted the head branch")],
        PullEventKind::HeadRestored => vec![Piece::words("restored the head branch")],
        PullEventKind::Labeled { label } => {
            vec![Piece::words("added"), Piece::Label(label.clone())]
        }
        PullEventKind::Unlabeled { label } => {
            vec![Piece::words("removed"), Piece::Label(label.clone())]
        }
        PullEventKind::Assigned { who } if same(who) => vec![Piece::words("self-assigned this")],
        PullEventKind::Assigned { who } => vec![Piece::words(format!("assigned this to {who}"))],
        PullEventKind::Unassigned { who } if same(who) => {
            vec![Piece::words("removed their assignment")]
        }
        PullEventKind::Unassigned { who } => vec![Piece::words(format!("unassigned {who}"))],
        PullEventKind::ReviewRequested { who } if same(who) => {
            vec![Piece::words("self-requested a review")]
        }
        PullEventKind::ReviewRequested { who } => {
            vec![Piece::words(format!("requested a review from {who}"))]
        }
        PullEventKind::ReviewRequestRemoved { who } => {
            vec![Piece::words(format!("removed the review request for {who}"))]
        }
        PullEventKind::ReviewDismissed { who } if who.is_empty() => {
            vec![Piece::words("dismissed a review")]
        }
        PullEventKind::ReviewDismissed { who } => {
            vec![Piece::words(format!("dismissed {who}'s review"))]
        }
        PullEventKind::Renamed { from, to } => {
            vec![Piece::words(format!("changed the title from \u{201c}{from}\u{201d} to \u{201c}{to}\u{201d}"))]
        }
        PullEventKind::Milestoned { title } => {
            vec![Piece::words(format!("added this to the {title} milestone"))]
        }
        PullEventKind::Demilestoned { title } => {
            vec![Piece::words(format!("removed this from the {title} milestone"))]
        }
        PullEventKind::Referenced { sha } => {
            vec![Piece::words("referenced this in"), Piece::Sha(sha.clone())]
        }
        PullEventKind::CrossReferenced { reference } => vec![
            Piece::words("mentioned this in"),
            Piece::Reference(reference.clone()),
        ],
        PullEventKind::Pinned => vec![Piece::words("pinned this")],
        PullEventKind::Unpinned => vec![Piece::words("unpinned this")],
        PullEventKind::Locked => vec![Piece::words("locked the conversation")],
        PullEventKind::Unlocked => vec![Piece::words("unlocked the conversation")],
        PullEventKind::AutoMergeEnabled => vec![Piece::words("enabled auto-merge")],
        PullEventKind::AutoMergeDisabled => vec![Piece::words("disabled auto-merge")],
        PullEventKind::MergeQueued => vec![Piece::words("added this to the merge queue")],
        PullEventKind::MergeDequeued => vec![Piece::words("removed this from the merge queue")],
        PullEventKind::CommentDeleted { author } if author.is_empty() => {
            vec![Piece::words("deleted a comment")]
        }
        PullEventKind::CommentDeleted { author } if same(author) => {
            vec![Piece::words("deleted their comment")]
        }
        PullEventKind::CommentDeleted { author } => {
            vec![Piece::words(format!("deleted a comment from {author}"))]
        }
    }
}

/// A commit's subject in runs, each with the file it names when `file`
/// finds one in it. A word names a file when, with the quotes, brackets
/// and sentence stops around it taken off, it has a dot or a slash in it
/// and `file` knows it; a word that is only a word, `Update`, is never
/// asked about, so a file called `build` does not turn every "build"
/// into a link. The runs put together are the subject.
pub fn subject_runs<T>(subject: &str, file: impl Fn(&str) -> Option<T>) -> Vec<(String, Option<T>)> {
    let mut runs: Vec<(String, Option<T>)> = Vec::new();
    let plain = |runs: &mut Vec<(String, Option<T>)>, text: &str| match runs.last_mut() {
        Some((last, None)) => last.push_str(text),
        _ if !text.is_empty() => runs.push((text.to_string(), None)),
        _ => {}
    };
    let mut rest = subject;
    while !rest.is_empty() {
        let start = rest.find(|c: char| !c.is_whitespace()).unwrap_or(rest.len());
        plain(&mut runs, &rest[..start]);
        rest = &rest[start..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..end];
        rest = &rest[end..];
        let head = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '`']);
        let core = head.trim_start_matches(['(', '[', '{', '"', '\'', '`']);
        let found = core.contains(['.', '/']).then(|| file(core)).flatten();
        match found {
            Some(found) => {
                let at = head.len() - core.len();
                plain(&mut runs, &word[..at]);
                runs.push((core.to_string(), Some(found)));
                plain(&mut runs, &word[at + core.len()..]);
            }
            None => plain(&mut runs, word),
        }
    }
    runs
}

/// The event's line as words, for the harness and for keying.
pub fn event_name(event: &PullEvent) -> String {
    event_pieces(event)
        .iter()
        .map(Piece::text)
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
/// with no body; an issue's from the issue, with its comments and the
/// events GitHub's timeline gave it. A bare branch has neither, and its
/// timeline is its commits over the base, as a pull request's would be,
/// and its threads.
pub fn timeline(
    pull: Option<&PullRequest>,
    discussion: Option<&Discussion>,
    issue: Option<(&Issue, &[IssueComment], &[PullEvent])>,
    branch_commits: &[Commit],
    threads: &[CommentThread],
) -> Vec<Card> {
    let mut cards = Vec::new();

    match (pull, issue) {
        (Some(_), _) => cards.push(Card::Description {
            at: discussion.and_then(|d| d.opened_at),
            body: discussion.map(|d| d.body.clone()).unwrap_or_default(),
            node_id: discussion.map(|d| d.node_id.clone()).unwrap_or_default(),
            editable: discussion.is_some_and(|d| d.viewer_can_update),
        }),
        (None, Some((issue, _, _))) => cards.push(Card::Description {
            at: issue.created_at,
            body: issue.body.clone(),
            node_id: issue.node_id.clone(),
            editable: issue.viewer_can_update,
        }),
        (None, None) => {}
    }

    if let Some(d) = discussion {
        cards.extend(d.comments.iter().cloned().map(Card::Comment));
        cards.extend(d.events.iter().cloned().map(Card::Event));
        cards.extend(review_cards(&d.reviews));
    }
    if let Some((_, comments, events)) = issue {
        cards.extend(comments.iter().cloned().map(Card::Comment));
        cards.extend(events.iter().cloned().map(Card::Event));
    }
    if pull.is_none() && issue.is_none() {
        cards.extend(branch_commits.iter().map(|commit| {
            Card::Event(PullEvent::new(
                PullEventKind::Committed {
                    sha: commit.sha.clone(),
                    author: commit.author.clone(),
                    login: String::new(),
                    subject: commit.subject.clone(),
                },
                commit.authored_at,
                "",
            ))
        }));
    }
    cards.extend(threads.iter().cloned().map(Card::Thread));

    // The sort is stable, so cards a list already had in order keep it.
    cards.sort_by_key(|c| (!matches!(c, Card::Description { .. }), c.at(), c.rank()));
    cards
}

/// The review cards. A pending review is the reader's own draft on
/// github.com and has not been said yet; a review that only said
/// "commented" with nothing in its body was made by GitHub for a line
/// comment, which is a thread of its own.
fn review_cards(reviews: &[Review]) -> Vec<Card> {
    reviews
        .iter()
        .filter(|review| review.state != "PENDING")
        .filter(|review| !(review.state == "COMMENTED" && review.body.trim().is_empty()))
        .map(|review| Card::Review { review: review.clone() })
        .collect()
}

/// The fixtures are shared with the harness's test.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use perch_github::{Anchor, Author, CommentMessage, ReviewComment, Side, ThreadOrigin};

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
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
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
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
            opened_at: Some(at("2026-09-14T14:02:00Z")),
            body: "The description.".into(),
            comments: vec![],
            reviews,
            review_comments,
            events: vec![],
            review_threads: Vec::new(),
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
            html_url: format!("https://github.com/a/b/pull/57#issuecomment-{id}"),
            viewer_can_delete: false,
            viewer_can_update: false,
        }
    }

    pub(crate) fn review(id: u64, state: &str, body: &str, when: Option<&str>) -> Review {
        Review {
            viewer_can_update: false,
            node_id: String::new(),
            reactions: Default::default(),
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
            node_id: String::new(),
            reactions: Default::default(),
            id,
            pull_request_review_id: review,
            in_reply_to_id: reply_to,
            user: "chris".into(),
            body: body.into(),
            path: "src/manifest.rs".into(),
            line: Some(41),
            original_line: None,
            start_line: None,
            original_start_line: None,
            start_side: None,
            commit_id: String::new(),
            original_commit_id: String::new(),
            user_avatar_url: String::new(),
            diff_hunk: String::new(),
            side: Side::Right,
            created_at: at(when),
            updated_at: None,
            html_url: String::new(),
            viewer_can_delete: false,
            viewer_can_update: false,
        }
    }

    pub(crate) fn event(kind: PullEventKind, when: &str) -> PullEvent {
        PullEvent::new(kind, at(when), "rohan")
    }

    /// A thread made at `created`, whose question was asked at `asked`.
    fn thread(id: &str, created: &str, asked: &str, question: &str) -> CommentThread {
        CommentThread {
            id: id.into(),
            worktree_id: "w".into(),
            origin: ThreadOrigin::Local,
            github_id: None,
            anchor: Some(Anchor { path: "src/x.rs".into(), start_line: 41, line: 58, side: Side::Right, commit_sha: "h".into() }),
            outdated: false,
            resolved: false,
            node_id: None,
            can_resolve: false,
            created_at: at(created),
            messages: vec![CommentMessage {
                id: format!("{id}-q"),
                thread_id: id.into(),
                origin: ThreadOrigin::Local,
                author: Author::User,
                body: question.into(),
                created_at: at(asked),
                run: None,
                github_id: None,
                node_id: None,
                html_url: None,
                diff_hunk: None,
                reactions: Default::default(),
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
    fn a_review_sorts_by_its_own_time_and_its_line_comments_are_not_in_it() {
        let mut d = discussion(
            vec![review(9, "APPROVED", "Looks right.", Some("2026-09-16T11:40:00Z"))],
            vec![
                line(31, Some(9), None, "2026-09-16T11:38:00Z", "epoch"),
                line(32, Some(9), None, "2026-09-16T12:10:00Z", "fence"),
            ],
        );
        d.comments.push(comment(1, "2026-09-16T12:00:00Z", "Thanks"));
        let cards = timeline(Some(&pull()), Some(&d), None, &[], &[]);
        assert_eq!(kinds(&cards), ["description", "review", "comment"]);
        assert_eq!(lines(&cards), ["The description.", "approved  Looks right.", "Thanks"]);
        let Card::Review { review } = &cards[1] else { panic!("{cards:?}") };
        assert_eq!(review.id, 9);
        assert_eq!(cards[1].at(), Some(at("2026-09-16T11:40:00Z")), "the review's own time");
    }

    #[test]
    fn a_github_thread_is_a_thread_card_at_its_first_comments_time() {
        let mut d = discussion(vec![review(9, "COMMENTED", "", Some("2026-09-16T11:40:00Z"))], vec![]);
        d.comments.push(comment(1, "2026-09-16T10:00:00Z", "first"));
        d.comments.push(comment(2, "2026-09-16T12:00:00Z", "second"));
        let mut t = thread("gh-31", "2026-09-16T11:38:00Z", "2026-09-16T11:38:00Z", "epoch");
        t.origin = ThreadOrigin::GitHub;
        t.github_id = Some(31);
        t.messages[0].origin = ThreadOrigin::GitHub;
        t.messages[0].author = Author::GitHubUser("rohan".into());
        t.messages[0].github_id = Some(31);
        let cards = timeline(Some(&pull()), Some(&d), None, &[], &[t]);
        assert_eq!(kinds(&cards), ["description", "comment", "thread", "comment"], "the empty review is noise");
        assert_eq!(cards[2].first_line(), "epoch");
    }

    #[test]
    fn a_thread_sorts_by_its_question_between_two_comments() {
        let mut d = discussion(vec![], vec![]);
        d.comments.push(comment(1, "2026-09-16T10:00:00Z", "first"));
        d.comments.push(comment(2, "2026-09-16T12:00:00Z", "second"));
        let t = thread("t1", "2026-09-16T09:30:00Z", "2026-09-16T11:00:00Z", "why the fence?");
        let cards = timeline(Some(&pull()), Some(&d), None, &[], &[t]);
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
            event(PullEventKind::Merged, "2026-09-17T15:00:00Z"),
        ];
        let cards = timeline(Some(&pull()), Some(&d), None, &[], &[]);
        assert_eq!(kinds(&cards), ["description", "event", "comment", "event", "comment"]);
        assert_eq!(lines(&cards)[1], "opened");
        assert_eq!(lines(&cards)[3], "merged", "an event wins a tie");
    }

    #[test]
    fn an_issue_has_a_description_comments_threads_and_its_own_events() {
        let issue = issue();
        let comments = vec![comment(1, "2026-09-15T10:00:00Z", "Seen it too.")];
        let t = thread("t1", "2026-09-16T09:00:00Z", "2026-09-16T09:00:00Z", "where?");
        let labelled = event(
            PullEventKind::Labeled { label: Label { name: "bug".into(), color: "d73a4a".into() } },
            "2026-09-15T09:00:00Z",
        );
        let cards = timeline(
            None,
            None,
            Some((&issue, &comments, &[labelled])),
            &[],
            std::slice::from_ref(&t),
        );
        assert_eq!(kinds(&cards), ["description", "event", "comment", "thread"]);
        assert_eq!(lines(&cards)[1], "added bug");

        let cards = timeline(None, None, Some((&issue, &comments, &[])), &[], &[t]);
        assert_eq!(kinds(&cards), ["description", "comment", "thread"]);
        let Card::Description { at: opened, body, .. } = &cards[0] else { panic!("{cards:?}") };
        assert_eq!(*opened, Some(at("2026-09-14T14:02:00Z")));
        assert_eq!(body, "The manifest can be written twice.");
    }

    #[test]
    fn the_description_is_first_even_when_a_thread_predates_it() {
        let d = discussion(vec![], vec![]);
        let t = [thread("t1", "2026-09-01T09:00:00Z", "2026-09-01T09:00:00Z", "early")];
        let cards = timeline(Some(&pull()), Some(&d), None, &[], &t);
        assert_eq!(kinds(&cards), ["description", "thread"]);

        // Without a discussion the description is the pull request alone.
        let cards = timeline(Some(&pull()), None, None, &[], &t);
        assert_eq!(kinds(&cards), ["description", "thread"]);
        assert_eq!(cards[0].at(), None);
        assert_eq!(cards[0].first_line(), "No description.");

        // A bare branch has no description at all.
        let cards = timeline(None, None, None, &[], &t);
        assert_eq!(kinds(&cards), ["thread"]);
    }

    #[test]
    fn a_bare_branch_shows_its_commits_among_its_threads() {
        let commit = |sha: &str, when: &str, subject: &str| Commit {
            sha: sha.into(),
            subject: subject.into(),
            body: "Why.".into(),
            author: "Chris".into(),
            authored_at: at(when),
        };
        // Newest first, as git lists them.
        let commits = [
            commit("bbbbbbbbbb", "2026-09-16T12:00:00Z", "Mirror the manifest"),
            commit("aaaaaaaaaa", "2026-09-16T10:00:00Z", "Fence the manifest"),
        ];
        let t = [thread("t1", "2026-09-16T11:00:00Z", "2026-09-16T11:00:00Z", "why fence?")];
        let cards = timeline(None, None, None, &commits, &t);
        assert_eq!(kinds(&cards), ["event", "thread", "event"]);
        assert_eq!(
            lines(&cards),
            ["committed aaaaaaa Fence the manifest", "why fence?", "committed bbbbbbb Mirror the manifest"]
        );

        // A pull request's commits come from GitHub's timeline instead.
        let cards = timeline(Some(&pull()), None, None, &commits, &t);
        assert_eq!(kinds(&cards), ["description", "thread"]);
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
        let cards = timeline(Some(&pull()), Some(&d), None, &[], &[]);
        assert_eq!(kinds(&cards), ["description", "review"], "a line comment no longer keeps its empty review");
        assert_eq!(lines(&cards)[1..], ["dismissed"]);
        let Card::Review { review } = &cards[1] else { panic!("{cards:?}") };
        assert_eq!(review.id, 4);
    }

    #[test]
    fn a_line_comment_with_no_review_in_the_list_is_not_a_card() {
        let d = discussion(vec![], vec![line(31, Some(77), None, "2026-09-16T11:38:00Z", "epoch")]);
        let cards = timeline(Some(&pull()), Some(&d), None, &[], &[]);
        assert_eq!(kinds(&cards), ["description"]);
    }

    #[test]
    fn a_review_without_a_time_sorts_first() {
        let mut d = discussion(vec![review(9, "APPROVED", "", None)], vec![]);
        d.comments.push(comment(1, "2026-09-16T11:00:00Z", "before"));
        d.comments.push(comment(2, "2026-09-16T12:00:00Z", "after"));
        let cards = timeline(Some(&pull()), Some(&d), None, &[], &[]);
        assert_eq!(lines(&cards), ["The description.", "approved", "before", "after"]);
    }

    #[test]
    fn an_events_line_names_what_was_done_and_not_who_did_it_twice() {
        let name = |kind| event_name(&event(kind, "2026-09-16T11:00:00Z"));
        // `event` is rohan's, so what rohan did to rohan is "self".
        assert_eq!(name(PullEventKind::Merged), "merged");
        assert_eq!(
            name(PullEventKind::Labeled {
                label: Label { name: "needs work".into(), color: String::new() }
            }),
            "added needs work"
        );
        assert_eq!(
            name(PullEventKind::Committed {
                sha: "abc1234def5678".into(),
                author: "Chris".into(),
                login: String::new(),
                subject: "Fence the manifest".into(),
            }),
            "committed abc1234 Fence the manifest",
            "seven characters of the sha"
        );
        assert_eq!(
            name(PullEventKind::ReviewRequested { who: "chris".into() }),
            "requested a review from chris"
        );
        assert_eq!(
            name(PullEventKind::ReviewRequested { who: "rohan".into() }),
            "self-requested a review"
        );
        assert_eq!(
            name(PullEventKind::Assigned { who: "rohan".into() }),
            "self-assigned this"
        );
        assert_eq!(
            name(PullEventKind::Assigned { who: "chris".into() }),
            "assigned this to chris"
        );
        assert_eq!(
            name(PullEventKind::ReviewDismissed { who: String::new() }),
            "dismissed a review"
        );
        assert_eq!(
            name(PullEventKind::CommentDeleted { author: "chris".into() }),
            "deleted a comment from chris"
        );
        assert_eq!(
            name(PullEventKind::CommentDeleted { author: "rohan".into() }),
            "deleted their comment"
        );
        assert_eq!(
            name(PullEventKind::CommentDeleted { author: String::new() }),
            "deleted a comment",
            "GitHub could not say whose"
        );
        assert_eq!(
            name(PullEventKind::ForcePushed { sha: "abc1234def".into() }),
            "force-pushed to abc1234"
        );
        assert_eq!(
            name(PullEventKind::CrossReferenced {
                reference: Reference {
                    repo_full_name: "slatedb/slatedb".into(),
                    number: 2098,
                    title: "Use remote_seq".into(),
                    pull: true,
                    html_url: String::new(),
                }
            }),
            "mentioned this in slatedb/slatedb#2098 Use remote_seq"
        );
    }

    #[test]
    fn a_file_a_subject_names_is_a_run_of_its_own() {
        let files = ["bindings/uniffi/src/config.rs", "README.md"];
        let known = |word: &str| files.contains(&word).then(|| word.to_string());
        let runs = |subject: &str| subject_runs(subject, known);
        assert_eq!(
            runs("Update bindings/uniffi/src/config.rs"),
            vec![
                ("Update ".into(), None),
                ("bindings/uniffi/src/config.rs".into(), Some("bindings/uniffi/src/config.rs".into())),
            ]
        );
        assert_eq!(
            runs("Fix (README.md), and `bindings/uniffi/src/config.rs`."),
            vec![
                ("Fix (".into(), None),
                ("README.md".into(), Some("README.md".into())),
                ("), and `".into(), None),
                ("bindings/uniffi/src/config.rs".into(), Some("bindings/uniffi/src/config.rs".into())),
                ("`.".into(), None),
            ]
        );
        assert_eq!(runs("Fence the manifest"), vec![("Fence the manifest".into(), None)]);
        assert_eq!(runs("Bump to v1.2"), vec![("Bump to v1.2".into(), None)], "not a file");
        assert_eq!(runs("  two  spaces "), vec![("  two  spaces ".into(), None)]);
        let asked = std::cell::RefCell::new(Vec::new());
        subject_runs("Update build and src/x.rs", |word| {
            asked.borrow_mut().push(word.to_string());
            None::<()>
        });
        assert_eq!(*asked.borrow(), ["src/x.rs"], "a bare word is never looked up");
    }

    #[test]
    fn the_pieces_of_a_line_are_what_the_view_draws_apart() {
        let commit = event(
            PullEventKind::Committed {
                sha: "abc1234def".into(),
                author: "Chris".into(),
                login: String::new(),
                subject: "Fence the manifest".into(),
            },
            "2026-09-16T11:00:00Z",
        );
        assert_eq!(
            event_pieces(&commit),
            vec![
                Piece::Text("committed".into()),
                Piece::Commit("abc1234def".into()),
                Piece::Subject("Fence the manifest".into()),
            ]
        );
        let label = Label { name: "bug".into(), color: "d73a4a".into() };
        assert_eq!(
            event_pieces(&event(
                PullEventKind::Unlabeled { label: label.clone() },
                "2026-09-16T11:00:00Z"
            )),
            vec![Piece::Text("removed".into()), Piece::Label(label)]
        );
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
        assert_eq!(Card::Event(event(PullEventKind::Draft, "2026-09-16T11:00:00Z")).first_line(), "converted this to draft");
        assert_eq!(verdict(&review(1, "CHANGES_REQUESTED", "", None)), "requested changes");
        assert_eq!(verdict(&review(1, "SOMETHING_NEW", "", None)), "commented");
    }
}
