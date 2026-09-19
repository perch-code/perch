//! The backfill.
//!
//! What the list pass makes rows for, this archives. Each round takes the
//! open items whose discussion has moved since it was last kept, newest
//! first, and then walks each declared repository's closed history,
//! oldest first, filing every discussion under knowledge and nowhere
//! else: a closed pull request nobody opened in Perch is a file an agent
//! can find, not a row. It runs beside the poll loop, a round after each
//! poll's passes, at the pace the allowance allows, and is a function
//! over its parts rather than over `Ctx` so the harness can run one round
//! in the foreground and the tests can run one against a fake GitHub.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::mpsc::Sender as EventSender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use perch_github::{Cache, ItemKind, ItemSubject, Page};
use perch_knowledge::{Knowledge, Message};

use super::sync::{with, Error, Source};
use super::{Ctx, Event};

/// Below this many requests left, the backfill waits for the reset
/// before asking anything more. The list pass and whatever the reader
/// does are never held by it; they take their requests as they come.
pub(crate) const FLOOR: i64 = 1000;
/// Added to every wait for a reset, so a clock a little behind GitHub's
/// does not wake into the window that was just used up.
pub(crate) const GRACE: Duration = Duration::from_secs(5);
/// The wait when GitHub says rate limited without saying until when.
const UNKNOWN_RESET: Duration = Duration::from_secs(5 * 60);
/// The least a rate limit answer is waited out. A reset already past
/// means GitHub is refusing on some count other than the window's, and
/// asking again in a few seconds would only add to it.
const LIMITED_AT_LEAST: Duration = Duration::from_secs(60);

/// How the backfill waits, so a test can pin a wait rather than take it.
pub(crate) trait Sleeper {
    async fn sleep(&self, wait: Duration);
}

/// The runtime's clock.
pub(crate) struct Clock;

impl Sleeper for Clock {
    async fn sleep(&self, wait: Duration) {
        tokio::time::sleep(wait).await
    }
}

/// What a backfill runs over. The app hands it the context's parts, the
/// harness its own, and the tests a fake GitHub, a temporary cache and a
/// temporary knowledge root.
pub(crate) struct Backfill<'a, S, W> {
    pub source: &'a S,
    pub cache: &'a Mutex<Cache>,
    pub knowledge: Arc<Knowledge>,
    /// Whether the skill files are rewritten as repositories appear.
    pub write_skill: bool,
    /// The repositories whose history is walked, in config order.
    pub declared: Vec<String>,
    pub events: EventSender<Event>,
    /// Items an open or a notification is fetching right now, shared with
    /// `refresh_open_item`, so the two never write one item at once. One
    /// found taken is passed over: it is being archived by the other, and
    /// it will be stale next round if it needs to be.
    pub refreshing: Arc<Mutex<HashSet<String>>>,
    /// Whether an item is the one open in the window. The open item is
    /// left to the open path: every notification about it refreshes and
    /// archives it there, and had the backfill taken the guard first,
    /// that refresh would have been dropped and the window left showing
    /// the conversation as it was.
    pub is_open: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub sleeper: W,
    /// Stop a round after this many discussions, for the harness.
    pub limit: Option<usize>,
}

/// The app's backfill, over the context's client, cache and knowledge,
/// woken by its passes. A context with no cache has no rows to read and
/// gets none.
pub(super) fn spawn(ctx: Ctx) {
    let Some(cache) = ctx.cache.clone() else {
        tracing::info!("no cache, so no backfill");
        return;
    };
    tokio::spawn(async move {
        let window = ctx.clone();
        let backfill = Backfill {
            source: ctx.github.as_ref(),
            cache: cache.as_ref(),
            knowledge: ctx.knowledge.clone(),
            write_skill: ctx.config.knowledge.skills,
            declared: ctx.config.declared(),
            events: ctx.events.clone(),
            refreshing: ctx.refreshing.clone(),
            is_open: Arc::new(move |id: &str| window.is_open(id)),
            sleeper: Clock,
            limit: None,
        };
        backfill.run(ctx.passes.clone()).await;
    });
}

impl<S: Source, W: Sleeper> Backfill<'_, S, W> {
    /// Rounds for as long as the app runs, one on each wake. A wake is a
    /// poll's passes finishing, and `Notify` keeps the permit from a wake
    /// nothing was waiting for, so the first pass's permit starts the
    /// first round and a round that outlasts a poll is followed by
    /// another as soon as it ends. A round that failed says so in the
    /// status bar and waits for the next wake like any other.
    pub async fn run(self, wake: Arc<tokio::sync::Notify>) {
        loop {
            wake.notified().await;
            match self.round(None).await {
                Ok(0) => tracing::debug!("backfill round: nothing to archive"),
                Ok(n) => tracing::info!(archived = n, "backfill round done"),
                Err(e) => {
                    tracing::warn!(error = %e, "backfill stopped; waiting for the next poll");
                    let _ = self.events.send(Event::Notice(format!("Could not archive a discussion: {e}")));
                }
            }
        }
    }

    /// One round: the stale open items, then each repository's history
    /// from where its cursor left off. `only` narrows both steps to one
    /// repository, for the harness. Returns how many discussions were
    /// archived. The status line comes down when the round ends, whether
    /// it ran to the end or the harness's limit stopped it.
    pub async fn round(&self, only: Option<&str>) -> Result<usize, Error> {
        let mut progress = Progress { events: &self.events, limit: self.limit, archived: 0, by_repo: HashMap::new() };
        let outcome = self.round_inner(only, &mut progress).await;
        Progress::over(&self.events);
        outcome.map(|()| progress.archived)
    }

    async fn round_inner(&self, only: Option<&str>, progress: &mut Progress<'_>) -> Result<(), Error> {
        self.open_items(only, progress).await?;
        let declared: Vec<&str> = match only {
            Some(repo) => vec![repo],
            None => self.declared.iter().map(String::as_str).collect(),
        };
        for repo in declared {
            for kind in [ItemKind::PullRequest, ItemKind::Issue] {
                if progress.full() {
                    return Ok(());
                }
                self.history(repo, kind, progress).await?;
            }
        }
        Ok(())
    }

    /// Step one: every item whose kept discussion is missing or older
    /// than its subject, newest subject first, so what is moving is
    /// archived before what is not. The rows are read once, up front, so
    /// the count each repository is told is the count it will get.
    async fn open_items(&self, only: Option<&str>, progress: &mut Progress<'_>) -> Result<(), Error> {
        let stale: Vec<(String, ItemSubject)> = with(self.cache, |c| {
            let mut stale = Vec::new();
            for id in c.discussion_stale()? {
                let Some(item) = c.load_item(&id)? else { continue };
                let Some(subject) = item.subject else { continue };
                if only.is_none_or(|r| subject.repo_full_name().eq_ignore_ascii_case(r)) {
                    stale.push((item.id, subject));
                }
            }
            Ok(stale)
        })?;
        for (_, subject) in &stale {
            progress.expect(subject.repo_full_name(), 1);
        }
        for (id, subject) in stale {
            if (self.is_open)(&id) {
                tracing::debug!(item = id, "open in the window; its own path archives it");
                continue;
            }
            let taken = self.refreshing.lock().map(|mut s| s.insert(id.clone())).unwrap_or(false);
            if !taken {
                tracing::info!(item = id, "being refreshed; left for the next round");
                continue;
            }
            let outcome = self.archive_item(&id, &subject).await;
            if let Ok(mut s) = self.refreshing.lock() {
                s.remove(&id);
            }
            let added = outcome?;
            progress.archived(subject.repo_full_name(), &subject, added);
            if progress.full() {
                return Ok(());
            }
        }
        Ok(())
    }

    /// One item's discussion fetched, kept on its row and filed. The row
    /// is looked for again before the write: an item deleted from the
    /// sidebar while GitHub answered has no row for a discussion to hang
    /// on, and its records are still worth filing.
    async fn archive_item(&self, id: &str, subject: &ItemSubject) -> Result<usize, Error> {
        let (discussion, messages) = self.fetch(subject).await?;
        with(self.cache, |c| match c.load_item(id)? {
            Some(_) => c.store_discussion(id, &discussion),
            None => Ok(()),
        })?;
        self.file(messages).await
    }

    /// Step two, one list of a repository's history: closed pull requests
    /// or closed issues, from the cursor. A page number walks the list
    /// from that page; a `done:` cursor catches up on what has been
    /// updated since the walk reached the end.
    async fn history(&self, repo: &str, kind: ItemKind, progress: &mut Progress<'_>) -> Result<(), Error> {
        let key = cursor_key(kind, repo);
        let cursor = with(self.cache, |c| c.meta(&key))?;
        match Cursor::parse(cursor.as_deref()) {
            Cursor::Page(from) => self.walk(repo, kind, from, &key, progress).await,
            Cursor::Done(since) => self.catch_up(repo, kind, since, &key, progress).await,
        }
    }

    /// The first walk over a list, oldest updated first, a page at a
    /// time. The page number is written to the cursor before each next
    /// page is asked, so a run stopped part way starts again at the page
    /// it was on, and a discussion or two from that page are archived
    /// twice, which ingest does not mind. After the last page the cursor
    /// records the newest `updated_at` seen, which is where the catch-up
    /// starts from.
    async fn walk(
        &self,
        repo: &str,
        kind: ItemKind,
        from: u32,
        key: &str,
        progress: &mut Progress<'_>,
    ) -> Result<(), Error> {
        let mut page = from;
        let mut newest: Option<Timestamp> = None;
        loop {
            let listed = self.page(repo, kind, page, None).await?;
            if page == from {
                // About how many there are: the `Link` header's last page
                // at a hundred a page, or, on the last page itself, the
                // pages before it and what is on it. The pages before
                // this one were done in an earlier session.
                let before = (from as usize - 1) * 100;
                let closed = listed.last_page.map(|l| l as usize * 100).unwrap_or(before + listed.items.len());
                progress.expect(repo, closed);
                progress.skip(repo, before);
            }
            let last = listed.last_page.is_none();
            for subject in listed.items {
                newest = newest.max(subject.updated_at());
                let added = self.file(self.fetch(&subject).await?.1).await?;
                progress.archived(repo, &subject, added);
                if progress.full() {
                    return Ok(());
                }
            }
            if last {
                break;
            }
            page += 1;
            with(self.cache, |c| c.set_meta(key, &page.to_string()))?;
        }
        let newest = newest.unwrap_or(Timestamp::UNIX_EPOCH);
        with(self.cache, |c| c.set_meta(key, &done(newest)))?;
        tracing::info!(repo, kind = kind.column(), newest = %newest, "history walked to the end");
        Ok(())
    }

    /// What has been updated since the walk ended. Issues take `since`,
    /// so the list is only what is new, paged through whole. Pull
    /// requests have no `since`: the first page says how many pages there
    /// are, and since the list is oldest first, what is new is at its
    /// end, read from the last page back to the first page that holds an
    /// entry already archived. Nothing is written to the cursor until the
    /// end, so a catch-up stopped part way is done again next round.
    async fn catch_up(
        &self,
        repo: &str,
        kind: ItemKind,
        since: Timestamp,
        key: &str,
        progress: &mut Progress<'_>,
    ) -> Result<(), Error> {
        let mut newest = since;
        let first = self.page(repo, kind, 1, Some(since)).await?;
        let last = first.last_page;
        self.catch_up_page(repo, first, since, &mut newest, progress).await?;
        match (kind, last) {
            (_, None) => {}
            (ItemKind::Issue, Some(last)) => {
                for page in 2..=last {
                    if progress.full() {
                        break;
                    }
                    let listed = self.page(repo, kind, page, Some(since)).await?;
                    self.catch_up_page(repo, listed, since, &mut newest, progress).await?;
                }
            }
            (ItemKind::PullRequest, Some(last)) => {
                for page in (2..=last).rev() {
                    if progress.full() {
                        break;
                    }
                    let listed = self.page(repo, kind, page, None).await?;
                    if self.catch_up_page(repo, listed, since, &mut newest, progress).await? {
                        break;
                    }
                }
            }
        }
        if newest > since && !progress.full() {
            with(self.cache, |c| c.set_meta(key, &done(newest)))?;
        }
        Ok(())
    }

    /// The entries of one catch-up page newer than the cursor, archived.
    /// Returns whether the page held one that was not, which for pull
    /// requests, read from the end, means every page before it is older
    /// still. For issues `since` has already left those out.
    async fn catch_up_page(
        &self,
        repo: &str,
        listed: Page<ItemSubject>,
        since: Timestamp,
        newest: &mut Timestamp,
        progress: &mut Progress<'_>,
    ) -> Result<bool, Error> {
        let (fresh, stale): (Vec<_>, Vec<_>) =
            listed.items.into_iter().partition(|s| s.updated_at().is_some_and(|t| t > since));
        progress.expect(repo, fresh.len());
        for subject in fresh {
            if let Some(t) = subject.updated_at() {
                *newest = (*newest).max(t);
            }
            let added = self.file(self.fetch(&subject).await?.1).await?;
            progress.archived(repo, &subject, added);
            if progress.full() {
                return Ok(true);
            }
        }
        Ok(!stale.is_empty())
    }

    /// One page of a list, as subjects, so the two lists go through one
    /// walk.
    async fn page(
        &self,
        repo: &str,
        kind: ItemKind,
        page: u32,
        since: Option<Timestamp>,
    ) -> Result<Page<ItemSubject>, Error> {
        match kind {
            ItemKind::PullRequest => {
                let p = self.paced(|| self.source.list_closed_pulls(repo, page)).await?;
                Ok(Page { items: p.items.into_iter().map(ItemSubject::PullRequest).collect(), last_page: p.last_page })
            }
            ItemKind::Issue => {
                let p = self.paced(|| self.source.list_closed_issues(repo, page, since)).await?;
                Ok(Page { items: p.items.into_iter().map(ItemSubject::Issue).collect(), last_page: p.last_page })
            }
        }
    }

    /// A subject's discussion from GitHub, in the shape the cache keeps
    /// and as the records knowledge files.
    async fn fetch(&self, subject: &ItemSubject) -> Result<(perch_github::Discussion, Vec<Message>), Error> {
        match subject {
            ItemSubject::PullRequest(pr) => {
                let discussion = self.paced(|| self.source.fetch_discussion(&pr.repo_full_name, pr.number)).await?;
                let messages = crate::knowledge::to_messages(pr, &discussion);
                Ok((discussion, messages))
            }
            ItemSubject::Issue(issue) => {
                let comments =
                    self.paced(|| self.source.fetch_issue_comments(&issue.repo_full_name, issue.number)).await?;
                let messages = crate::knowledge::issue_to_messages(issue, &comments);
                Ok((super::issue_discussion(issue, &comments), messages))
            }
        }
    }

    /// The records under the knowledge root. Returns how many were new or
    /// changed.
    async fn file(&self, messages: Vec<Message>) -> Result<usize, Error> {
        super::ingest_into(self.knowledge.clone(), self.write_skill, messages).await.map_err(Error::Knowledge)
    }

    /// One request at the pace the allowance allows. Under the floor, the
    /// wait for the reset comes first. A rate limit answer is waited out
    /// the same way, at least a minute, or for five minutes when GitHub
    /// does not say, and the request is asked again. Anything else stops
    /// the round.
    async fn paced<T, Fut>(&self, ask: impl Fn() -> Fut) -> Result<T, Error>
    where
        Fut: Future<Output = Result<T, perch_github::Error>>,
    {
        loop {
            if let Some(allowance) = self.source.allowance() {
                if allowance.remaining < FLOOR {
                    let wait = until(allowance.reset_epoch);
                    tracing::info!(remaining = allowance.remaining, secs = wait.as_secs(), "under the floor; waiting for the reset");
                    self.sleeper.sleep(wait).await;
                }
            }
            match ask().await {
                Ok(answer) => return Ok(answer),
                Err(perch_github::Error::RateLimited { reset_epoch }) => {
                    let wait = reset_epoch.map(|r| until(r).max(LIMITED_AT_LEAST)).unwrap_or(UNKNOWN_RESET);
                    tracing::warn!(secs = wait.as_secs(), "rate limited; waiting before asking again");
                    self.sleeper.sleep(wait).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// The wait until a reset, and a little past it.
fn until(reset_epoch: i64) -> Duration {
    let now = Timestamp::now().as_second();
    Duration::from_secs((reset_epoch - now).max(0) as u64) + GRACE
}

/// Where a list's walk is: the page to ask next, or done and the newest
/// `updated_at` seen, to catch up from. A cursor that does not read is
/// the start, which costs a walk and loses nothing.
#[derive(Debug, PartialEq, Eq)]
enum Cursor {
    Page(u32),
    Done(Timestamp),
}

impl Cursor {
    fn parse(cursor: Option<&str>) -> Cursor {
        let Some(cursor) = cursor else { return Cursor::Page(1) };
        match cursor.strip_prefix("done:") {
            Some(newest) => newest.parse().map(Cursor::Done).unwrap_or(Cursor::Page(1)),
            None => cursor.parse().map(Cursor::Page).unwrap_or(Cursor::Page(1)),
        }
    }
}

/// The cursor value after a walk reached the end.
fn done(newest: Timestamp) -> String {
    format!("done:{newest}")
}

/// The meta key a list's cursor is kept under: `backfill:pulls:<owner/repo>`
/// or `backfill:issues:<owner/repo>`, the name lower-cased so a repository
/// spelled two ways in config is one key.
fn cursor_key(kind: ItemKind, repo: &str) -> String {
    let list = match kind {
        ItemKind::PullRequest => "pulls",
        ItemKind::Issue => "issues",
    };
    format!("backfill:{list}:{}", repo.to_lowercase())
}

/// The counts the status bar shows, by repository, told after each
/// discussion: how many are done of about how many. A repository's total
/// is its open items to archive plus what its history's first page said,
/// and it grows as the round learns more.
struct Progress<'a> {
    events: &'a EventSender<Event>,
    limit: Option<usize>,
    /// Discussions archived this round, over every repository.
    archived: usize,
    /// By lower-cased repository name.
    by_repo: HashMap<String, Count>,
}

/// One repository's count, under the name it was first seen by.
struct Count {
    repo: String,
    done: usize,
    total: usize,
}

impl Progress<'_> {
    fn count(&mut self, repo: &str) -> &mut Count {
        self.by_repo
            .entry(repo.to_lowercase())
            .or_insert_with(|| Count { repo: repo.to_string(), done: 0, total: 0 })
    }

    /// This many more to do for the repository.
    fn expect(&mut self, repo: &str, more: usize) {
        self.count(repo).total += more;
    }

    /// This many were done for the repository before this round.
    fn skip(&mut self, repo: &str, done: usize) {
        self.count(repo).done += done;
    }

    /// One discussion filed: the window hears which, and where the
    /// repository's count is. The count is kept under the name the round
    /// works by, the declared one for history, so a repository GitHub
    /// answers under another name is one line and not two.
    fn archived(&mut self, repo: &str, subject: &ItemSubject, added: usize) {
        self.archived += 1;
        let _ = self.events.send(Event::Archived {
            repo_full_name: subject.repo_full_name().to_string(),
            kind: subject.kind(),
            number: subject.number(),
            added,
        });
        let count = self.count(repo);
        count.done += 1;
        let (repo, done, total) = (count.repo.clone(), count.done, count.total);
        let _ = self.events.send(Event::Backfill { repo, done, total });
    }

    /// Whether the harness's limit has been reached.
    fn full(&self) -> bool {
        self.limit.is_some_and(|n| self.archived >= n)
    }

    /// The round is over, so the line comes down.
    fn over(events: &EventSender<Event>) {
        let _ = events.send(Event::Backfill { repo: String::new(), done: 0, total: 0 });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::mpsc::Receiver;

    use super::super::fake::{
        at, cache, closed_issue, comment, discussion, issue, merged, meta, page, pull, store_issue, store_pull, Fake,
        REPO,
    };
    use super::*;

    /// A sleeper that takes no time and plays the reset: it writes down
    /// the wait and gives the fake a full allowance, as the window
    /// turning over would.
    struct Pinned {
        fake: Arc<Fake>,
        waits: Mutex<Vec<Duration>>,
    }

    impl Sleeper for Pinned {
        async fn sleep(&self, wait: Duration) {
            self.fake.note(format!("sleep {}s", wait.as_secs()));
            self.waits.lock().unwrap().push(wait);
            self.fake.set_allowance(5000, Timestamp::now().as_second() + 3600);
        }
    }

    /// A backfill over a fake, a temporary cache and a temporary knowledge
    /// root, and the events it sent.
    struct Bench {
        fake: Arc<Fake>,
        _dir: tempfile::TempDir,
        cache: Mutex<Cache>,
        knowledge: Arc<Knowledge>,
        events: EventSender<Event>,
        received: Receiver<Event>,
        refreshing: Arc<Mutex<HashSet<String>>>,
        open: Option<String>,
        limit: Option<usize>,
    }

    impl Bench {
        fn new(fake: Fake) -> Self {
            let (dir, cache) = cache();
            let knowledge = Arc::new(Knowledge::open(dir.path().join("knowledge")));
            let (events, received) = std::sync::mpsc::channel();
            Bench {
                fake: Arc::new(fake),
                _dir: dir,
                cache,
                knowledge,
                events,
                received,
                refreshing: Default::default(),
                open: None,
                limit: None,
            }
        }

        fn backfill(&self) -> Backfill<'_, Fake, Pinned> {
            self.backfill_over(&self.fake)
        }

        /// The same over another source, for a test that wraps the fake.
        fn backfill_over<'a, S: Source>(&'a self, source: &'a S) -> Backfill<'a, S, Pinned> {
            let open = self.open.clone();
            Backfill {
                source,
                cache: &self.cache,
                knowledge: self.knowledge.clone(),
                write_skill: false,
                declared: vec![REPO.to_string()],
                events: self.events.clone(),
                refreshing: self.refreshing.clone(),
                is_open: Arc::new(move |id: &str| open.as_deref() == Some(id)),
                sleeper: Pinned { fake: self.fake.clone(), waits: Mutex::new(Vec::new()) },
                limit: self.limit,
            }
        }

        async fn round(&self) -> usize {
            self.backfill().round(None).await.unwrap()
        }

        fn calls(&self) -> Vec<String> {
            self.fake.calls()
        }

        /// The rendered file for a pull request or an issue, if the
        /// round filed one.
        fn filed(&self, kind: &str, n: u64) -> Option<String> {
            std::fs::read_to_string(self.knowledge.root().join("a/b/github").join(kind).join(format!("{n}.md"))).ok()
        }

        fn backfill_events(&self) -> Vec<(String, usize, usize)> {
            self.received
                .try_iter()
                .filter_map(|e| match e {
                    Event::Backfill { repo, done, total } => Some((repo, done, total)),
                    _ => None,
                })
                .collect()
        }
    }

    fn discussions(numbers: impl IntoIterator<Item = u64>) -> HashMap<u64, perch_github::Discussion> {
        numbers.into_iter().map(|n| (n, discussion(n))).collect()
    }

    fn comments(numbers: impl IntoIterator<Item = u64>) -> HashMap<u64, Vec<perch_github::IssueComment>> {
        numbers.into_iter().map(|n| (n, vec![comment(n * 10, &format!("comment on issue {n}"))])).collect()
    }

    /// A repository with nothing closed, so a test of step one has no
    /// history to walk.
    fn no_history(fake: Fake) -> Fake {
        Fake {
            closed_pulls: HashMap::from([(1, page(vec![], None))]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            ..fake
        }
    }

    #[tokio::test]
    async fn the_open_items_are_archived_before_the_history_newest_first() {
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([(1, page(vec![merged(3, "2026-09-01T00:00:00Z")], None))]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            discussions: discussions([3, 7, 8]),
            comments: comments([42]),
            ..Default::default()
        });
        store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        store_pull(&bench.cache, pull(8, "2026-09-17T09:00:00Z"));
        store_issue(&bench.cache, issue(42, "2026-09-16T20:00:00Z"));

        let archived = bench.round().await;

        assert_eq!(archived, 4);
        assert_eq!(
            bench.calls(),
            vec![
                "discussion 8",
                "comments 42",
                "discussion 7",
                "closed pulls page=1",
                "discussion 3",
                "closed issues page=1 since=-",
            ],
            "the newest open item first, and the history only after every open one"
        );
        // The open items' discussions are kept on their rows, so the next
        // open paints before the network answers, and are no longer stale.
        let stale = bench.cache.lock().unwrap().discussion_stale().unwrap();
        assert!(stale.is_empty(), "{stale:?}");
        assert!(bench.filed("pulls", 8).unwrap().contains("comment on 8"));
        assert!(bench.filed("issues", 42).unwrap().contains("comment on issue 42"));
    }

    #[tokio::test]
    async fn a_closed_page_lands_in_knowledge_and_not_in_items() {
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([(1, page(vec![merged(3, "2026-09-01T00:00:00Z")], None))]),
            closed_issues: HashMap::from([(1, page(vec![closed_issue(5, "2026-09-02T00:00:00Z")], None))]),
            discussions: discussions([3]),
            comments: comments([5]),
            ..Default::default()
        });

        bench.round().await;

        assert!(bench.filed("pulls", 3).unwrap().contains("comment on 3"));
        assert!(bench.filed("issues", 5).unwrap().contains("comment on issue 5"));
        assert!(bench.cache.lock().unwrap().load_items().unwrap().is_empty(), "history is not a row");
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-01T00:00:00Z"));
        assert_eq!(meta(&bench.cache, "backfill:issues:a/b").as_deref(), Some("done:2026-09-02T00:00:00Z"));
    }

    #[tokio::test]
    async fn the_page_cursor_is_written_before_the_next_page_and_a_restart_resumes_there() {
        // Three pages of closed pull requests; the discussion on page
        // two's second entry cannot be fetched, so the walk stops there.
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([
                (1, page(vec![merged(1, "2026-09-01T00:00:00Z")], Some(3))),
                (2, page(vec![merged(2, "2026-09-02T00:00:00Z"), merged(3, "2026-09-03T00:00:00Z")], Some(3))),
                (3, page(vec![merged(4, "2026-09-04T00:00:00Z")], None)),
            ]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            discussions: discussions([1, 2]),
            ..Default::default()
        });

        let err = bench.backfill().round(None).await.unwrap_err();

        assert!(matches!(err, Error::GitHub(_)), "{err}");
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("2"), "the page it was on");
        assert_eq!(bench.calls(), vec!["closed pulls page=1", "discussion 1", "closed pulls page=2", "discussion 2", "discussion 3"]);

        // The next run, with the discussion answering, starts at page two:
        // page one is not asked again, and its entry is not refetched.
        let bench = Bench {
            fake: Arc::new(Fake {
                closed_pulls: HashMap::from([
                    (2, page(vec![merged(2, "2026-09-02T00:00:00Z"), merged(3, "2026-09-03T00:00:00Z")], Some(3))),
                    (3, page(vec![merged(4, "2026-09-04T00:00:00Z")], None)),
                ]),
                closed_issues: HashMap::from([(1, page(vec![], None))]),
                discussions: discussions([2, 3, 4]),
                ..Default::default()
            }),
            ..bench
        };

        let archived = bench.round().await;

        assert_eq!(archived, 3, "page two over again, then page three");
        assert_eq!(
            bench.calls(),
            vec![
                "closed pulls page=2",
                "discussion 2",
                "discussion 3",
                "closed pulls page=3",
                "discussion 4",
                "closed issues page=1 since=-",
            ]
        );
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-04T00:00:00Z"));
    }

    #[tokio::test]
    async fn after_the_last_page_a_later_run_passes_since_for_issues_and_skips_old_pulls() {
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([(1, page(vec![merged(1, "2026-09-01T00:00:00Z")], None))]),
            closed_issues: HashMap::from([(1, page(vec![closed_issue(2, "2026-09-02T00:00:00Z")], None))]),
            discussions: discussions([1]),
            comments: comments([2]),
            ..Default::default()
        });
        bench.round().await;
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-01T00:00:00Z"));
        assert_eq!(meta(&bench.cache, "backfill:issues:a/b").as_deref(), Some("done:2026-09-02T00:00:00Z"));

        // Since then #3 merged and #4 was reopened and closed, and issue
        // #5 closed. GitHub's issues list with `since` still holds the
        // boundary entry; the pull request list holds everything.
        let bench = Bench {
            fake: Arc::new(Fake {
                closed_pulls: HashMap::from([(
                    1,
                    page(
                        vec![merged(1, "2026-09-01T00:00:00Z"), merged(3, "2026-09-05T00:00:00Z"), merged(4, "2026-09-06T00:00:00Z")],
                        None,
                    ),
                )]),
                closed_issues: HashMap::from([(
                    1,
                    page(vec![closed_issue(2, "2026-09-02T00:00:00Z"), closed_issue(5, "2026-09-07T00:00:00Z")], None),
                )]),
                discussions: discussions([3, 4]),
                comments: comments([5]),
                ..Default::default()
            }),
            ..bench
        };

        let archived = bench.round().await;

        assert_eq!(archived, 3);
        assert_eq!(
            bench.calls(),
            vec![
                "closed pulls page=1",
                "discussion 3",
                "discussion 4",
                "closed issues page=1 since=2026-09-02T00:00:00Z",
                "comments 5",
            ],
            "the old pull request and the boundary issue are not fetched again"
        );
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-06T00:00:00Z"));
        assert_eq!(meta(&bench.cache, "backfill:issues:a/b").as_deref(), Some("done:2026-09-07T00:00:00Z"));
    }

    #[tokio::test]
    async fn a_catch_up_over_many_pages_of_pulls_reads_the_end_back_to_the_first_old_entry() {
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([
                (1, page(vec![merged(1, "2026-09-01T00:00:00Z")], Some(4))),
                (2, page(vec![merged(2, "2026-09-02T00:00:00Z")], Some(4))),
                (3, page(vec![merged(3, "2026-09-03T00:00:00Z"), merged(5, "2026-09-05T00:00:00Z")], Some(4))),
                (4, page(vec![merged(6, "2026-09-06T00:00:00Z")], None)),
            ]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            discussions: discussions([5, 6]),
            ..Default::default()
        });
        bench.cache.lock().unwrap().set_meta("backfill:pulls:a/b", "done:2026-09-04T00:00:00Z").unwrap();
        bench.cache.lock().unwrap().set_meta("backfill:issues:a/b", "done:2026-09-04T00:00:00Z").unwrap();

        let archived = bench.round().await;

        assert_eq!(archived, 2);
        assert_eq!(
            bench.calls(),
            vec![
                "closed pulls page=1",
                "closed pulls page=4",
                "discussion 6",
                "closed pulls page=3",
                "discussion 5",
                "closed issues page=1 since=2026-09-04T00:00:00Z",
            ],
            "page two, older than the cursor throughout, is never asked for"
        );
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-06T00:00:00Z"));
    }

    #[tokio::test]
    async fn under_the_floor_the_wait_ends_at_the_reset_and_the_fetch_follows() {
        let bench = Bench::new(no_history(Fake { discussions: discussions([7]), ..Default::default() }));
        store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        let before = Timestamp::now().as_second();
        let reset = before + 100;
        bench.fake.set_allowance(FLOOR - 1, reset);

        let backfill = bench.backfill();
        let archived = backfill.round(None).await.unwrap();
        let after = Timestamp::now().as_second();

        assert_eq!(archived, 1);
        let waits = backfill.sleeper.waits.lock().unwrap().clone();
        assert_eq!(waits.len(), 1, "once: the sleeper played the reset");
        let wait = waits[0].as_secs() as i64;
        let lowest = reset - after + GRACE.as_secs() as i64;
        let highest = reset - before + GRACE.as_secs() as i64;
        assert!((lowest..=highest).contains(&wait), "{wait}s ends at the reset and a little past it");
        let calls = bench.calls();
        assert!(calls[0].starts_with("sleep "), "the wait before the fetch: {calls:?}");
        assert_eq!(calls[1], "discussion 7");
    }

    #[tokio::test]
    async fn a_rate_limit_answer_is_waited_out_and_the_request_asked_again() {
        // The fake refuses #7 as rate limited once, then answers.
        struct Once {
            inner: Arc<Fake>,
            refused: Mutex<bool>,
        }
        impl Source for Once {
            async fn list_open_pulls(&self, r: &str, e: Option<&str>) -> Result<perch_github::Listed<perch_github::PullRequest>, perch_github::Error> {
                self.inner.list_open_pulls(r, e).await
            }
            async fn list_open_issues(&self, r: &str, e: Option<&str>) -> Result<perch_github::Listed<perch_github::Issue>, perch_github::Error> {
                self.inner.list_open_issues(r, e).await
            }
            async fn list_closed_pulls(&self, r: &str, p: u32) -> Result<Page<perch_github::PullRequest>, perch_github::Error> {
                self.inner.list_closed_pulls(r, p).await
            }
            async fn list_closed_issues(&self, r: &str, p: u32, s: Option<Timestamp>) -> Result<Page<perch_github::Issue>, perch_github::Error> {
                self.inner.list_closed_issues(r, p, s).await
            }
            async fn fetch_pull_request(&self, r: &str, n: u64) -> Result<perch_github::PullRequest, perch_github::Error> {
                self.inner.fetch_pull_request(r, n).await
            }
            async fn fetch_issue(&self, r: &str, n: u64) -> Result<perch_github::Issue, perch_github::Error> {
                self.inner.fetch_issue(r, n).await
            }
            async fn fetch_discussion(&self, r: &str, n: u64) -> Result<perch_github::Discussion, perch_github::Error> {
                let first = std::mem::replace(&mut *self.refused.lock().unwrap(), false);
                if first {
                    self.inner.note(format!("discussion {n} refused"));
                    return Err(perch_github::Error::RateLimited { reset_epoch: None });
                }
                self.inner.fetch_discussion(r, n).await
            }
            async fn fetch_issue_comments(&self, r: &str, n: u64) -> Result<Vec<perch_github::IssueComment>, perch_github::Error> {
                self.inner.fetch_issue_comments(r, n).await
            }
            fn allowance(&self) -> Option<perch_github::Allowance> {
                self.inner.allowance()
            }
        }

        let bench = Bench::new(no_history(Fake { discussions: discussions([7]), ..Default::default() }));
        store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        let once = Once { inner: bench.fake.clone(), refused: Mutex::new(true) };
        let backfill = bench.backfill_over(&once);

        let archived = backfill.round(None).await.unwrap();

        assert_eq!(archived, 1);
        assert_eq!(backfill.sleeper.waits.lock().unwrap().as_slice(), [UNKNOWN_RESET], "five minutes when GitHub does not say");
        assert_eq!(bench.calls()[..3], ["discussion 7 refused".to_string(), "sleep 300s".to_string(), "discussion 7".to_string()]);
    }

    #[tokio::test]
    async fn an_item_being_refreshed_is_skipped_and_one_open_in_the_window_is_left_to_its_own_path() {
        let bench = Bench::new(no_history(Fake { discussions: discussions([7, 8, 9]), ..Default::default() }));
        let refreshing = store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        let open = store_pull(&bench.cache, pull(8, "2026-09-16T11:40:00Z"));
        store_pull(&bench.cache, pull(9, "2026-09-16T11:40:00Z"));
        bench.refreshing.lock().unwrap().insert(refreshing.id.clone());
        let bench = Bench { open: Some(open.id.clone()), ..bench };

        let archived = bench.round().await;

        assert_eq!(archived, 1);
        assert_eq!(bench.calls()[0], "discussion 9", "only the one nobody else is writing");
        assert!(bench.filed("pulls", 7).is_none());
        assert!(bench.filed("pulls", 8).is_none());
        let stale = bench.cache.lock().unwrap().discussion_stale().unwrap();
        assert_eq!(stale, vec![refreshing.id.clone(), open.id.clone()], "both still stale for the next round");
        assert!(bench.refreshing.lock().unwrap().contains(&refreshing.id), "the guard is the other's to release");
    }

    #[tokio::test]
    async fn the_backfill_event_carries_the_counts_and_ends_with_total_zero() {
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([
                (1, page(vec![merged(1, "2026-09-01T00:00:00Z")], Some(2))),
                (2, page(vec![merged(2, "2026-09-02T00:00:00Z")], None)),
            ]),
            closed_issues: HashMap::from([(1, page(vec![closed_issue(5, "2026-09-02T00:00:00Z")], None))]),
            discussions: discussions([1, 2, 7]),
            comments: comments([5]),
            ..Default::default()
        });
        store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));

        bench.round().await;

        let repo = REPO.to_string();
        assert_eq!(
            bench.backfill_events(),
            vec![
                // Step one: one open item to do.
                (repo.clone(), 1, 1),
                // Two pages at a hundred a page, as the Link header has it.
                (repo.clone(), 2, 201),
                (repo.clone(), 3, 201),
                // One page of issues, so exactly one more.
                (repo.clone(), 4, 202),
                (String::new(), 0, 0),
            ]
        );
    }

    #[tokio::test]
    async fn a_resumed_walk_counts_the_pages_already_done() {
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([(3, page(vec![merged(9, "2026-09-09T00:00:00Z")], None))]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            discussions: discussions([9]),
            ..Default::default()
        });
        bench.cache.lock().unwrap().set_meta("backfill:pulls:a/b", "3").unwrap();

        bench.round().await;

        assert_eq!(bench.backfill_events()[0], (REPO.to_string(), 201, 201), "two pages before this one");
    }

    #[tokio::test]
    async fn the_harness_limit_stops_the_round_and_leaves_the_cursor_on_the_page() {
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([
                (1, page(vec![merged(1, "2026-09-01T00:00:00Z"), merged(2, "2026-09-02T00:00:00Z")], Some(2))),
                (2, page(vec![merged(3, "2026-09-03T00:00:00Z")], None)),
            ]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            discussions: discussions([1, 2, 3, 7]),
            ..Default::default()
        });
        store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        let bench = Bench { limit: Some(2), ..bench };

        let archived = bench.round().await;

        assert_eq!(archived, 2);
        assert_eq!(bench.calls(), vec!["discussion 7", "closed pulls page=1", "discussion 1"]);
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b"), None, "page one, to be read again");
        assert_eq!(bench.backfill_events().last(), Some(&(String::new(), 0, 0)), "the line comes down");
    }

    #[tokio::test]
    async fn only_one_repository_when_the_harness_names_it() {
        let bench = Bench::new(no_history(Fake { discussions: discussions([7]), ..Default::default() }));
        store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        let mut elsewhere = pull(8, "2026-09-17T09:00:00Z");
        elsewhere.repo_full_name = "c/d".into();
        let mut other = perch_github::Item::new("c/d", at("2026-09-10T00:00:00Z"));
        other.subject = Some(ItemSubject::PullRequest(elsewhere));
        bench.cache.lock().unwrap().insert_item(&other).unwrap();

        let archived = bench.backfill().round(Some("A/B")).await.unwrap();

        assert_eq!(archived, 1);
        assert_eq!(bench.calls(), vec!["discussion 7", "closed pulls page=1", "closed issues page=1 since=-"]);
    }

    /// The task as the app runs it, on a runtime with worker threads so
    /// the test can block on the events it sends.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_task_runs_a_round_on_each_wake_and_a_failed_round_is_a_notice() {
        let (dir, cache) = cache();
        let cache = Arc::new(cache);
        store_pull(&cache, pull(7, "2026-09-16T11:40:00Z"));
        // No discussion for #7, so every round fails on it.
        let fake = Arc::new(no_history(Fake::default()));
        let knowledge = Arc::new(Knowledge::open(dir.path().join("knowledge")));
        let (events, received) = std::sync::mpsc::channel();
        let wake = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn({
            let (fake, cache, wake) = (fake.clone(), cache.clone(), wake.clone());
            async move {
                let backfill = Backfill {
                    source: fake.as_ref(),
                    cache: cache.as_ref(),
                    knowledge,
                    write_skill: false,
                    declared: vec![REPO.to_string()],
                    events,
                    refreshing: Default::default(),
                    is_open: Arc::new(|_: &str| false),
                    sleeper: Pinned { fake: fake.clone(), waits: Mutex::new(Vec::new()) },
                    limit: None,
                };
                backfill.run(wake).await
            }
        });
        let notice = || loop {
            match received.recv_timeout(Duration::from_secs(10)).expect("a round's events") {
                Event::Notice(message) => break message,
                _ => continue,
            }
        };

        wake.notify_one();
        let first = notice();
        wake.notify_one();
        let second = notice();
        task.abort();

        assert!(first.starts_with("Could not archive a discussion"), "{first}");
        assert_eq!(first, second, "and the next wake tried again");
        assert_eq!(fake.calls()[..2], ["discussion 7".to_string(), "discussion 7".to_string()]);
    }

    #[test]
    fn a_cursor_reads_as_a_page_or_as_done_and_anything_else_as_the_start() {
        assert_eq!(Cursor::parse(None), Cursor::Page(1));
        assert_eq!(Cursor::parse(Some("7")), Cursor::Page(7));
        assert_eq!(Cursor::parse(Some("done:2026-09-01T00:00:00Z")), Cursor::Done(at("2026-09-01T00:00:00Z")));
        assert_eq!(Cursor::parse(Some("done:soon")), Cursor::Page(1));
        assert_eq!(Cursor::parse(Some("page three")), Cursor::Page(1));
        assert_eq!(cursor_key(ItemKind::Issue, "SlateDB/SlateDB"), "backfill:issues:slatedb/slatedb");
    }
}
