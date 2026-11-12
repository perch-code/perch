//! An unread notification opens on what it is about: Conversation
//! scrolled to the card with the newest activity GitHub woke the reader
//! for, a comment, a review, a reply in a thread or an event such as a
//! merge, and the card flashes.
//!
//! The discussion is painted from the cache first and GitHub's answer
//! follows, so the page waits for a card about as new as the
//! notification before it moves. When none comes, as when what happened
//! is not on the page, it goes to the newest it has after a moment. A
//! reader who scrolls first is left where they are.

use std::cell::Cell;
use std::time::{Duration, Instant};

use floem::reactive::{Effect, SignalGet, SignalUpdate, SignalWith};
use perch_github::{ItemKind, Thread};

use crate::review::cards;
use crate::review::conversation;
use crate::review::doc::CardId;
use crate::review::model::{ItemTab, ReviewState};
use crate::review::timeline::{self, Card, NOTIFIED_SLACK};
use crate::state::AppState;

/// How long the page waits for GitHub's answer to bring a card as new as
/// the notification, once it has any card to go to.
const WAIT: Duration = Duration::from_secs(4);

/// How long an open waits at all. An item that never had a card to go to
/// does not jump at the reader much later, when a poll brings one.
const GIVE_UP: Duration = Duration::from_secs(30);

/// The notification being opened, and the page's wait for it.
#[derive(Debug, Clone)]
pub struct Notified {
    thread: Thread,
    opened: Instant,
    /// Which wait the timer is for, once one has begun.
    wait: Option<u64>,
}

thread_local! {
    static WAITS: Cell<u64> = const { Cell::new(0) };
}

/// A notification's row was clicked to open its pull request or issue.
/// Only an unread one is followed: a read one opens where the reader
/// left it.
pub fn follow(review: ReviewState, thread: &Thread) {
    if !thread.unread {
        return;
    }
    review.notified.set(Some(Notified {
        thread: thread.clone(),
        opened: Instant::now(),
        wait: None,
    }));
}

/// A pull request or an issue was opened from a row that shows its
/// notification's unread dot, in Worktrees or Browse: as from the
/// notification's own row.
pub fn follow_subject(app: AppState, repo_full_name: &str, kind: ItemKind, number: u64) {
    let Some(review) = app.review.get_untracked() else {
        return;
    };
    if let Some(thread) = app.unread_thread(repo_full_name, kind, number) {
        follow(review, &thread);
    }
}

/// The reader scrolled the page: the open is theirs now.
pub fn cancel(review: ReviewState) {
    if review.notified.with_untracked(Option::is_some) {
        review.notified.set(None);
    }
}

/// Look again whenever what the page shows changes, while an open is
/// waiting. For the window's life.
pub fn wire(review: ReviewState) {
    Effect::new(move |_| {
        if review.notified.with(Option::is_none) {
            return;
        }
        let cards = conversation::cards(review);
        Effect::untrack(|| settle(review, &cards, false));
    });
}

/// Go to the card, if the open item is the notification's and the page
/// has caught up with it or has waited long enough.
pub(crate) fn settle(review: ReviewState, cards: &[Card], waited: bool) {
    let Some(notified) = review.notified.get_untracked() else {
        return;
    };
    if notified.opened.elapsed() > GIVE_UP {
        review.notified.set(None);
        return;
    }
    let about = review.item.with_untracked(|item| {
        item.as_ref().map(|item| {
            item.subject.as_ref().is_some_and(|s| {
                notified
                    .thread
                    .is_about(s.repo_full_name(), s.kind(), s.number())
            })
        })
    });
    match about {
        // Still opening.
        None => return,
        Some(false) => {
            review.notified.set(None);
            return;
        }
        Some(true) => {}
    }
    let updated_at = notified.thread.updated_at;
    let Some((card, at)) = timeline::notified_card(cards, updated_at) else {
        return;
    };
    let caught_up = at
        .checked_add(NOTIFIED_SLACK)
        .is_ok_and(|at| at >= updated_at);
    if !caught_up && !waited {
        if notified.wait.is_none() {
            start_wait(review);
        }
        return;
    }
    let card = cards::card_id(card);
    review.notified.set(None);
    go_to(review, card);
}

/// After `WAIT`, the newest card the page has by then, if this is still
/// the open waiting.
fn start_wait(review: ReviewState) {
    let wait = WAITS.with(|w| {
        w.set(w.get() + 1);
        w.get()
    });
    review.notified.update(|n| {
        if let Some(n) = n {
            n.wait = Some(wait);
        }
    });
    floem::action::exec_after(WAIT, move |_| {
        // The window may have closed in the meantime.
        let current = review
            .notified
            .try_with_untracked(|n| n.and_then(|n| n.as_ref()?.wait));
        if current == Some(wait) {
            let cards = Effect::untrack(|| conversation::cards(review));
            settle(review, &cards, true);
        }
    });
}

/// Conversation up, the card open if it is a folded thread, and asked to
/// come into view once it is laid out.
pub(crate) fn go_to(review: ReviewState, card: CardId) {
    if let CardId::Thread(id) = &card {
        if review.is_folded(id) {
            review.toggle_fold(id);
        }
    }
    if review.tab.get_untracked() != ItemTab::Conversation {
        // Where the reader left the page is not where they are going.
        review.conversation_scroll.set(0.0);
        review.show_tab(ItemTab::Conversation);
    }
    review.pending_focus_card.set(Some(card));
    if let Some(page) = review.conversation_page.get_untracked() {
        page.request_layout();
    }
}

#[cfg(test)]
mod tests {
    use floem::reactive::{SignalGet, SignalUpdate};
    use perch_github::{ItemKind, Reason, Repository, Subject, SubjectKind, Thread};

    use crate::review::model::ReviewState;
    use crate::state::AppState;

    fn thread(number: u64, unread: bool) -> Thread {
        Thread {
            id: format!("n{number}"),
            unread,
            local_unread: false,
            local_read: false,
            reason: Reason::Comment,
            updated_at: "2026-09-18T09:00:20Z".parse().unwrap(),
            last_read_at: None,
            subject: Subject {
                title: "Fence the manifest".into(),
                kind: SubjectKind::PullRequest,
                url: Some(format!(
                    "https://api.github.com/repos/criccomini/perch/pulls/{number}"
                )),
                latest_comment_url: None,
            },
            repository: Repository {
                full_name: "criccomini/perch".into(),
                owner_avatar_url: String::new(),
            },
            subject_snapshot: None,
            run_snapshot: None,
            snoozed_until: None,
        }
    }

    /// An item's row with the unread dot opens as its notification's row
    /// does; one whose notification is read, or that has none, does not.
    #[test]
    fn a_subject_is_followed_only_while_its_notification_is_unread() {
        let app = AppState::new();
        let review = ReviewState::new();
        app.review.set(Some(review));
        app.notifications
            .set(vec![thread(57, true), thread(58, false)]);

        super::follow_subject(app, "criccomini/perch", ItemKind::PullRequest, 58);
        assert!(review.notified.get().is_none(), "read");
        super::follow_subject(app, "criccomini/perch", ItemKind::PullRequest, 59);
        assert!(review.notified.get().is_none(), "no notification");
        super::follow_subject(app, "criccomini/perch", ItemKind::Issue, 57);
        assert!(review.notified.get().is_none(), "another kind");
        super::follow_subject(app, "Criccomini/Perch", ItemKind::PullRequest, 57);
        assert_eq!(
            review.notified.get().map(|n| n.thread.id),
            Some("n57".to_string())
        );
    }
}
