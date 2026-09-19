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
/// gets none. A local repository has no history on GitHub to walk, and
/// is left out of the list.
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
            declared: ctx.config.declared().into_iter().filter(|r| !perch_github::is_local(r)).collect(),
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
            // The wait for the reset comes before the guard is taken. Held
            // across it, the guard would turn away the refresh of an item
            // the reader opened during the wait, and that refresh is not
            // asked for again.
            self.await_floor().await;
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
            let added = match outcome {
                Ok(added) => added,
                // Deleted, or in a repository the token can no longer see:
                // GitHub will answer the same way every round, so the row
                // is settled rather than left to stop every round here.
                Err(Error::GitHub(perch_github::Error::Status { status: status @ (404 | 410), .. })) => {
                    tracing::warn!(item = id, status, "GitHub no longer answers for it; what is kept stands");
                    self.settle(&id, &subject)?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            progress.archived(subject.repo_full_name(), &subject, added);
            if progress.full() {
                return Ok(());
            }
        }
        Ok(())
    }

    /// One item's discussion fetched, filed, and then kept on its row. The
    /// filing comes first: it is the row's `fetched_at` that makes the
    /// item stale, and a filing that failed must leave it stale so the
    /// next round does it again. The row is looked for again before the
    /// write: an item deleted from the sidebar while GitHub answered has
    /// no row for a discussion to hang on, and its records are still
    /// worth filing.
    async fn archive_item(&self, id: &str, subject: &ItemSubject) -> Result<usize, Error> {
        let (discussion, messages) = self.fetch(subject).await?;
        let added = self.file(messages).await?;
        with(self.cache, |c| match c.load_item(id)? {
            Some(_) => c.store_discussion(id, &discussion),
            None => Ok(()),
        })?;
        Ok(added)
    }

    /// A row for a subject GitHub no longer answers for, made current: the
    /// discussion kept on it stands, and a row with none gets an empty one,
    /// dated now, so the row is no longer stale and is not asked for every
    /// round. Nothing is filed; there is nothing to file.
    fn settle(&self, id: &str, subject: &ItemSubject) -> Result<(), Error> {
        with(self.cache, |c| {
            if c.load_item(id)?.is_none() {
                return Ok(());
            }
            let kept = match c.load_discussion(id)? {
                Some((_, kept)) => kept,
                None => match subject {
                    ItemSubject::Issue(issue) => super::issue_discussion(issue, &[]),
                    ItemSubject::PullRequest(_) => perch_github::Discussion::default(),
                },
            };
            c.store_discussion(id, &kept)
        })
    }

    /// Step two, one list of a repository's history: closed pull requests
    /// or closed issues, from the cursor. A page number walks the list
    /// from that page; a `done:` cursor catches up on what has been
    /// updated since the walk reached the end.
    async fn history(&self, repo: &str, kind: ItemKind, progress: &mut Progress<'_>) -> Result<(), Error> {
        let key = cursor_key(kind, repo);
        let cursor = with(self.cache, |c| c.meta(&key))?;
        match Cursor::parse(cursor.as_deref()) {
            Cursor::Page { page, skip } => self.walk(repo, kind, page, skip, HashMap::new(), progress).await,
            Cursor::Done(since) => match kind {
                ItemKind::PullRequest => self.catch_up_pulls(repo, since, progress).await,
                ItemKind::Issue => self.catch_up_issues(repo, since, progress).await,
            },
        }
    }

    /// A walk over a list from a page, oldest updated first, a page at a
    /// time: the first walk, and the tail of the pull requests a catch-up
    /// found new. The page number is written to the cursor before each
    /// next page is asked, so a run stopped part way starts again at the
    /// page it was on, and a discussion or two from that page are archived
    /// twice, which ingest does not mind. A run the harness's limit stops
    /// inside a page writes how far into it, and the next run skips that
    /// many. After the last page the cursor records the newest
    /// `updated_at` seen, which is where the catch-up starts from. Pages
    /// in `read` were fetched already and are taken from there.
    ///
    /// The list is read by page number, so an entry updated while the
    /// walk is past it moves to the end and shifts what follows back by
    /// one, and the next page starts one entry late. That entry is missed
    /// until it moves again or is opened; the milestone's page cursor
    /// cannot express anything else, and the README says so.
    async fn walk(
        &self,
        repo: &str,
        kind: ItemKind,
        from: u32,
        skip: usize,
        mut read: HashMap<u32, Page<ItemSubject>>,
        progress: &mut Progress<'_>,
    ) -> Result<(), Error> {
        let key = cursor_key(kind, repo);
        let mut page = from;
        let mut skip = skip;
        let mut newest: Option<Timestamp> = None;
        loop {
            let listed = match read.remove(&page) {
                Some(listed) => listed,
                None => self.page(repo, kind, page, None).await?,
            };
            if page == from {
                // About how many there are: the `Link` header's last page
                // at a hundred a page, or, on the last page itself, the
                // pages before it and what is on it. The pages before
                // this one, and the skipped entries of this one, were done
                // in an earlier run.
                let before = (from as usize - 1) * 100;
                let closed = listed.last_page.map(|l| l as usize * 100).unwrap_or(before + listed.items.len());
                progress.expect(repo, closed);
                progress.skip(repo, before + skip);
            }
            let last = listed.last_page.is_none();
            // Over everything on the page, the skipped entries included, so
            // a walk resumed at the end of the last page still ends where
            // the list does.
            newest = newest.max(listed.newest);
            let count = listed.items.len();
            for (i, subject) in listed.items.into_iter().enumerate().skip(skip) {
                let added = self.file(self.fetch(&subject).await?.1).await?;
                progress.archived(repo, &subject, added);
                if progress.full() && i + 1 < count {
                    with(self.cache, |c| c.set_meta(&key, &within(page, i + 1)))?;
                    return Ok(());
                }
            }
            skip = 0;
            if last {
                break;
            }
            page += 1;
            with(self.cache, |c| c.set_meta(&key, &page.to_string()))?;
            if progress.full() {
                return Ok(());
            }
        }
        let newest = newest.unwrap_or(Timestamp::UNIX_EPOCH);
        with(self.cache, |c| c.set_meta(&key, &done(newest)))?;
        tracing::info!(repo, kind = kind.column(), newest = %newest, "history walked to the end");
        Ok(())
    }

    /// What has been updated since the walk over the pull requests ended.
    /// They have no `since`, but the list is oldest first, so what is new
    /// is its tail: the first page says how many pages there are, and the
    /// pages are read from the last back to the first that holds an entry
    /// already archived, which is where the tail begins. The tail is then
    /// walked forward from there, oldest first, as a first walk is, with
    /// the pages just read taken from memory rather than asked again, and
    /// the cursor a page number while it goes, so a run stopped part way
    /// goes on from where it stopped rather than over again.
    async fn catch_up_pulls(&self, repo: &str, since: Timestamp, progress: &mut Progress<'_>) -> Result<(), Error> {
        let kind = ItemKind::PullRequest;
        let fresh = |s: &ItemSubject| s.updated_at().is_some_and(|t| t > since);
        let mut read = HashMap::new();
        let mut page = 1;
        let mut listed = self.page(repo, kind, page, None).await?;
        if let Some(last) = listed.last_page.filter(|l| *l > 1) {
            if !listed.items.iter().any(fresh) {
                page = last;
                listed = self.page(repo, kind, page, None).await?;
                // A page with nothing old on it began after the tail did;
                // page one, read already, was all old.
                while page > 2 && listed.items.iter().all(fresh) {
                    read.insert(page, listed);
                    page -= 1;
                    listed = self.page(repo, kind, page, None).await?;
                }
            }
        }
        let (start, skip) = match listed.items.iter().position(fresh) {
            Some(skip) => {
                read.insert(page, listed);
                (page, skip)
            }
            // Nothing new on this page: the tail begins on the one after
            // it, when there is one, and there is nothing new at all when
            // there is not.
            None if read.contains_key(&(page + 1)) => (page + 1, 0),
            None => return Ok(()),
        };
        self.walk(repo, kind, start, skip, read, progress).await
    }

    /// What has been updated since the walk over the issues ended. They
    /// take `since`, so the list is only what is new, oldest first, paged
    /// through whole. The cursor is written at the end, or, when the
    /// harness's limit stops the catch-up part way, to the newest second
    /// every entry of which has been passed: an entry sharing a second
    /// with the last one archived may be next, and `since` under a cursor
    /// at that second would leave it out. A limit smaller than the number
    /// of entries updated in one second cannot get past that second; a
    /// round without a limit, which is what the app runs, does not stop.
    async fn catch_up_issues(&self, repo: &str, since: Timestamp, progress: &mut Progress<'_>) -> Result<(), Error> {
        let kind = ItemKind::Issue;
        let key = cursor_key(kind, repo);
        let mut seen = Seen { latest: since, behind: since };
        let mut page = 1;
        let mut last = None;
        loop {
            let listed = self.page(repo, kind, page, Some(since)).await?;
            if page == 1 {
                last = listed.last_page;
            }
            let fresh: Vec<_> =
                listed.items.into_iter().filter(|s| s.updated_at().is_some_and(|t| t > since)).collect();
            progress.expect(repo, fresh.len());
            for (i, subject) in fresh.iter().enumerate() {
                if let Some(t) = subject.updated_at() {
                    seen.note(t);
                }
                let added = self.file(self.fetch(subject).await?.1).await?;
                progress.archived(repo, subject, added);
                if progress.full() && i + 1 < fresh.len() {
                    let next = fresh[i + 1].updated_at();
                    return self.stop_short(&key, seen.passed(next), since);
                }
            }
            // The page was read past its last issue when a pull request
            // came after it; the cursor moves past that too.
            if let Some(t) = listed.newest {
                seen.note(t);
            }
            if !last.is_some_and(|l| page < l) {
                break;
            }
            if progress.full() {
                return self.stop_short(&key, seen.passed(None), since);
            }
            page += 1;
        }
        if seen.latest > since {
            with(self.cache, |c| c.set_meta(&key, &done(seen.latest)))?;
        }
        Ok(())
    }

    /// The cursor after a catch-up the limit stopped, when it is past
    /// where the catch-up started.
    fn stop_short(&self, key: &str, passed: Timestamp, since: Timestamp) -> Result<(), Error> {
        if passed > since {
            with(self.cache, |c| c.set_meta(key, &done(passed)))?;
        }
        Ok(())
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
                Ok(Page {
                    items: p.items.into_iter().map(ItemSubject::PullRequest).collect(),
                    last_page: p.last_page,
                    newest: p.newest,
                })
            }
            ItemKind::Issue => {
                let p = self.paced(|| self.source.list_closed_issues(repo, page, since)).await?;
                Ok(Page {
                    items: p.items.into_iter().map(ItemSubject::Issue).collect(),
                    last_page: p.last_page,
                    newest: p.newest,
                })
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

    /// The wait under the floor: until the reset the client last saw, and
    /// a little past it. Nothing, when the allowance is not known or is
    /// above the floor.
    async fn await_floor(&self) {
        if let Some(allowance) = self.source.allowance() {
            if allowance.remaining < FLOOR {
                let wait = until(allowance.reset_epoch);
                tracing::info!(remaining = allowance.remaining, secs = wait.as_secs(), "under the floor; waiting for the reset");
                self.sleeper.sleep(wait).await;
            }
        }
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
            self.await_floor().await;
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

/// Where a list's walk is: the page to ask next and how many of its
/// entries a run stopped inside it has done, or done and the newest
/// `updated_at` seen, to catch up from. A cursor that does not read is
/// the start, which costs a walk and loses nothing.
#[derive(Debug, PartialEq, Eq)]
enum Cursor {
    Page { page: u32, skip: usize },
    Done(Timestamp),
}

impl Cursor {
    const START: Cursor = Cursor::Page { page: 1, skip: 0 };

    fn parse(cursor: Option<&str>) -> Cursor {
        let Some(cursor) = cursor else { return Cursor::START };
        if let Some(newest) = cursor.strip_prefix("done:") {
            return newest.parse().map(Cursor::Done).unwrap_or(Cursor::START);
        }
        let (page, skip) = cursor.split_once('+').unwrap_or((cursor, "0"));
        match (page.parse(), skip.parse()) {
            (Ok(page), Ok(skip)) => Cursor::Page { page, skip },
            _ => Cursor::START,
        }
    }
}

/// The cursor value for a walk stopped inside a page: the page, and how
/// many of its entries are done.
fn within(page: u32, skip: usize) -> String {
    format!("{page}+{skip}")
}

/// The cursor value after a walk reached the end.
fn done(newest: Timestamp) -> String {
    format!("done:{newest}")
}

/// The newest `updated_at` an issues catch-up has seen, and the newest
/// before it. The list is oldest first, so once `latest` has moved on
/// from `behind`, every entry updated at or before `behind` has been
/// passed, whatever shares `latest`'s second further on.
struct Seen {
    latest: Timestamp,
    behind: Timestamp,
}

impl Seen {
    fn note(&mut self, t: Timestamp) {
        if t > self.latest {
            self.behind = self.latest;
            self.latest = t;
        }
    }

    /// The newest second every entry of which has been passed, given
    /// when the next entry was updated: `latest` once the list has moved
    /// on from it, and the one before while it has not, or when the next
    /// entry is on a page not yet read.
    fn passed(&self, next: Option<Timestamp>) -> Timestamp {
        if next.is_some_and(|n| n > self.latest) {
            self.latest
        } else {
            self.behind
        }
    }
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
    async fn a_catch_up_over_many_pages_of_pulls_reads_the_end_back_to_the_first_old_entry_then_archives_forward() {
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
                "closed pulls page=3",
                "discussion 5",
                "discussion 6",
                "closed issues page=1 since=2026-09-04T00:00:00Z",
            ],
            "page two, older than the cursor throughout, is never asked for; the tail is archived oldest first from the pages read"
        );
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-06T00:00:00Z"));
    }

    #[tokio::test]
    async fn a_catch_up_whose_tail_begins_on_a_page_boundary_walks_from_that_page() {
        // Page two is old throughout and page three new throughout: the
        // tail is page three from its first entry.
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([
                (1, page(vec![merged(1, "2026-09-01T00:00:00Z")], Some(3))),
                (2, page(vec![merged(2, "2026-09-02T00:00:00Z")], Some(3))),
                (3, page(vec![merged(5, "2026-09-05T00:00:00Z"), merged(6, "2026-09-06T00:00:00Z")], None)),
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
                "closed pulls page=3",
                "closed pulls page=2",
                "discussion 5",
                "discussion 6",
                "closed issues page=1 since=2026-09-04T00:00:00Z",
            ]
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
    async fn a_subject_github_no_longer_answers_for_is_settled_and_the_round_goes_on() {
        // Issue #42, the newest stale item, was deleted on GitHub; #7 and
        // the history behind it are still archived, and #42 is not asked
        // for again next round.
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([(1, page(vec![merged(3, "2026-09-01T00:00:00Z")], None))]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            discussions: discussions([3, 7]),
            refused: HashMap::from([(42, 404)]),
            ..Default::default()
        });
        store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        let gone = store_issue(&bench.cache, issue(42, "2026-09-16T20:00:00Z"));

        let archived = bench.round().await;

        assert_eq!(archived, 2, "the two that answered");
        assert_eq!(bench.calls(), vec!["comments 42", "discussion 7", "closed pulls page=1", "discussion 3", "closed issues page=1 since=-"]);
        let stale = bench.cache.lock().unwrap().discussion_stale().unwrap();
        assert!(stale.is_empty(), "settled: {stale:?}");
        let (_, kept) = bench.cache.lock().unwrap().load_discussion(&gone.id).unwrap().unwrap();
        assert!(kept.comments.is_empty(), "nothing invented for it");
        assert!(bench.filed("issues", 42).is_none(), "and nothing filed");

        bench.fake.calls.lock().unwrap().clear();
        bench.round().await;
        assert!(!bench.calls().iter().any(|c| c == "comments 42"), "{:?}", bench.calls());
    }

    #[tokio::test]
    async fn a_discussion_that_could_not_be_filed_stays_stale_and_is_done_again_next_round() {
        let bench = Bench::new(no_history(Fake { discussions: discussions([7]), ..Default::default() }));
        let item = store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        // A regular file where the knowledge root should be: nothing can
        // be written under it.
        std::fs::write(bench.knowledge.root(), "").unwrap();

        let err = bench.backfill().round(None).await.unwrap_err();

        assert!(matches!(err, Error::Knowledge(_)), "{err}");
        assert_eq!(bench.cache.lock().unwrap().discussion_stale().unwrap(), vec![item.id.clone()], "not kept as current");
        assert!(bench.cache.lock().unwrap().load_discussion(&item.id).unwrap().is_none());

        // With the root writable again, the next round files it.
        std::fs::remove_file(bench.knowledge.root()).unwrap();
        let archived = bench.round().await;

        assert_eq!(archived, 1);
        assert_eq!(bench.calls()[..2], ["discussion 7".to_string(), "discussion 7".to_string()]);
        assert!(bench.filed("pulls", 7).unwrap().contains("comment on 7"));
        assert!(bench.cache.lock().unwrap().discussion_stale().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_wait_for_the_reset_comes_before_the_guard_so_an_open_during_it_is_left_to_its_own_path() {
        // A sleeper that, while the wait goes on, has the reader open the
        // item the backfill was about to archive.
        struct Opens {
            fake: Arc<Fake>,
            open: Arc<Mutex<Option<String>>>,
            id: String,
        }
        impl Sleeper for Opens {
            async fn sleep(&self, wait: Duration) {
                self.fake.note(format!("sleep {}s", wait.as_secs()));
                *self.open.lock().unwrap() = Some(self.id.clone());
                self.fake.set_allowance(5000, Timestamp::now().as_second() + 3600);
            }
        }

        let bench = Bench::new(no_history(Fake { discussions: discussions([7]), ..Default::default() }));
        let item = store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        bench.fake.set_allowance(FLOOR - 1, Timestamp::now().as_second() + 100);
        let open = Arc::new(Mutex::new(None));
        let backfill = Backfill {
            source: bench.fake.as_ref(),
            cache: &bench.cache,
            knowledge: bench.knowledge.clone(),
            write_skill: false,
            declared: vec![REPO.to_string()],
            events: bench.events.clone(),
            refreshing: bench.refreshing.clone(),
            is_open: Arc::new({
                let open = open.clone();
                move |id: &str| open.lock().unwrap().as_deref() == Some(id)
            }),
            sleeper: Opens { fake: bench.fake.clone(), open, id: item.id.clone() },
            limit: None,
        };

        let archived = backfill.round(None).await.unwrap();

        assert_eq!(archived, 0, "left to the open path");
        assert_eq!(bench.calls()[0], "sleep 105s");
        assert!(!bench.calls().iter().any(|c| c == "discussion 7"), "not fetched: {:?}", bench.calls());
        assert!(bench.refreshing.lock().unwrap().is_empty(), "the guard was never taken, so a refresh of the open item was not turned away");
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
    async fn the_harness_limit_stops_the_round_and_the_next_run_goes_on_from_where_it_stopped() {
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([
                (
                    1,
                    page(
                        vec![merged(1, "2026-09-01T00:00:00Z"), merged(2, "2026-09-02T00:00:00Z"), merged(3, "2026-09-03T00:00:00Z")],
                        Some(2),
                    ),
                ),
                (2, page(vec![merged(4, "2026-09-04T00:00:00Z")], None)),
            ]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            discussions: discussions([1, 2, 3, 4, 7]),
            ..Default::default()
        });
        store_pull(&bench.cache, pull(7, "2026-09-16T11:40:00Z"));
        let bench = Bench { limit: Some(2), ..bench };

        // The open item and the first closed entry: stopped inside page one.
        let archived = bench.round().await;

        assert_eq!(archived, 2);
        assert_eq!(bench.calls(), vec!["discussion 7", "closed pulls page=1", "discussion 1"]);
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("1+1"), "one entry into page one");
        assert_eq!(bench.backfill_events().last(), Some(&(String::new(), 0, 0)), "the line comes down");

        // The next run skips that entry and finishes the page, and is
        // stopped at its end: the cursor is the next page.
        bench.fake.calls.lock().unwrap().clear();
        let archived = bench.round().await;

        assert_eq!(archived, 2, "nothing open is stale any more, and #1 is not archived again");
        assert_eq!(bench.calls(), vec!["closed pulls page=1", "discussion 2", "discussion 3"]);
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("2"));
        assert_eq!(bench.backfill_events()[0], (REPO.to_string(), 2, 200), "the skipped entry counts as done");

        // And the run after reaches the end.
        bench.fake.calls.lock().unwrap().clear();
        let archived = bench.round().await;

        assert_eq!(archived, 1);
        assert_eq!(bench.calls(), vec!["closed pulls page=2", "discussion 4", "closed issues page=1 since=-"]);
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-04T00:00:00Z"));
        assert!(bench.filed("pulls", 4).is_some());
    }

    #[tokio::test]
    async fn a_walk_resumed_past_the_last_entry_of_the_last_page_still_ends_where_the_list_does() {
        // The limit stopped an earlier run on page two's first entry, of
        // which there is only one, and nothing has changed since; the
        // page's newest is the cursor even though nothing on it is walked.
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([(2, page(vec![merged(3, "2026-09-03T00:00:00Z")], None))]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            ..Default::default()
        });
        bench.cache.lock().unwrap().set_meta("backfill:pulls:a/b", "2+1").unwrap();

        let archived = bench.round().await;

        assert_eq!(archived, 0);
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-03T00:00:00Z"));
    }

    #[tokio::test]
    async fn a_limited_catch_up_over_pulls_goes_on_from_where_it_stopped_next_run() {
        // Three merged since the cursor, across the last two pages, and a
        // limit of one: three runs, each one further, none over again.
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([
                (1, page(vec![merged(1, "2026-09-01T00:00:00Z")], Some(3))),
                (2, page(vec![merged(2, "2026-09-02T00:00:00Z"), merged(5, "2026-09-05T00:00:00Z")], Some(3))),
                (3, page(vec![merged(6, "2026-09-06T00:00:00Z"), merged(7, "2026-09-07T00:00:00Z")], None)),
            ]),
            closed_issues: HashMap::from([(1, page(vec![], None))]),
            discussions: discussions([5, 6, 7]),
            ..Default::default()
        });
        bench.cache.lock().unwrap().set_meta("backfill:pulls:a/b", "done:2026-09-04T00:00:00Z").unwrap();
        bench.cache.lock().unwrap().set_meta("backfill:issues:a/b", "done:2026-09-04T00:00:00Z").unwrap();
        let bench = Bench { limit: Some(1), ..bench };

        assert_eq!(bench.round().await, 1);
        assert_eq!(
            bench.calls(),
            vec!["closed pulls page=1", "closed pulls page=3", "closed pulls page=2", "discussion 5"],
            "the end read back to the first old entry, then the tail walked from there, oldest first"
        );
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("3"), "page two was finished");

        bench.fake.calls.lock().unwrap().clear();
        assert_eq!(bench.round().await, 1);
        assert_eq!(bench.calls(), vec!["closed pulls page=3", "discussion 6"]);
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("3+1"));

        bench.fake.calls.lock().unwrap().clear();
        assert_eq!(bench.round().await, 1);
        assert_eq!(bench.calls(), vec!["closed pulls page=3", "discussion 7"], "the limit is met at the end, so the issues wait");
        assert_eq!(meta(&bench.cache, "backfill:pulls:a/b").as_deref(), Some("done:2026-09-07T00:00:00Z"));
    }

    #[tokio::test]
    async fn a_limited_catch_up_over_issues_moves_the_cursor_to_the_last_whole_second_behind_it() {
        // Four closed since the cursor, two of them in one second, and a
        // limit of two. The first run stops after the first of the pair:
        // the cursor is the second before, since the other of the pair
        // would be left out by `since` under a cursor at its own time.
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([(1, page(vec![], None))]),
            closed_issues: HashMap::from([(
                1,
                page(
                    vec![
                        closed_issue(2, "2026-09-02T00:00:00Z"),
                        closed_issue(5, "2026-09-05T00:00:00Z"),
                        closed_issue(6, "2026-09-06T00:00:00Z"),
                        closed_issue(7, "2026-09-06T00:00:00Z"),
                        closed_issue(8, "2026-09-08T00:00:00Z"),
                    ],
                    None,
                ),
            )]),
            comments: comments([5, 6, 7, 8]),
            ..Default::default()
        });
        bench.cache.lock().unwrap().set_meta("backfill:pulls:a/b", "done:2026-09-02T00:00:00Z").unwrap();
        bench.cache.lock().unwrap().set_meta("backfill:issues:a/b", "done:2026-09-02T00:00:00Z").unwrap();
        let bench = Bench { limit: Some(2), ..bench };

        assert_eq!(bench.round().await, 2);
        assert_eq!(bench.calls(), vec!["closed pulls page=1", "closed issues page=1 since=2026-09-02T00:00:00Z", "comments 5", "comments 6"]);
        assert_eq!(meta(&bench.cache, "backfill:issues:a/b").as_deref(), Some("done:2026-09-05T00:00:00Z"));

        // The next run lists from there: #5 is the boundary and is not
        // fetched again, #6 is, and #7 is reached.
        bench.fake.calls.lock().unwrap().clear();
        assert_eq!(bench.round().await, 2);
        assert_eq!(bench.calls(), vec!["closed pulls page=1", "closed issues page=1 since=2026-09-05T00:00:00Z", "comments 6", "comments 7"]);
        assert_eq!(meta(&bench.cache, "backfill:issues:a/b").as_deref(), Some("done:2026-09-06T00:00:00Z"));

        bench.fake.calls.lock().unwrap().clear();
        assert_eq!(bench.round().await, 1);
        assert_eq!(bench.calls(), vec!["closed pulls page=1", "closed issues page=1 since=2026-09-06T00:00:00Z", "comments 8"]);
        assert_eq!(meta(&bench.cache, "backfill:issues:a/b").as_deref(), Some("done:2026-09-08T00:00:00Z"));
    }

    #[tokio::test]
    async fn the_issues_cursor_moves_past_the_pull_requests_the_issues_list_carried() {
        // The issues endpoint lists pull requests too and the client drops
        // them, but they count toward the page and toward `since`. Here
        // every entry of the closed list is a pull request but one, and the
        // pull requests were updated after it.
        let mut only_pulls = page(vec![], None);
        only_pulls.newest = Some(at("2026-09-09T00:00:00Z"));
        let bench = Bench::new(Fake {
            closed_pulls: HashMap::from([(1, page(vec![], None))]),
            closed_issues: HashMap::from([
                (1, page(vec![closed_issue(2, "2026-09-02T00:00:00Z")], Some(2))),
                (2, only_pulls.clone()),
            ]),
            comments: comments([2]),
            ..Default::default()
        });

        bench.round().await;

        assert_eq!(
            meta(&bench.cache, "backfill:issues:a/b").as_deref(),
            Some("done:2026-09-09T00:00:00Z"),
            "the walk ended past the pull requests, not at the last issue"
        );

        // Catching up from there lists nothing, and the cursor stays. Were
        // it at the issue, every page of pull requests since would be read
        // on every round.
        let bench = Bench {
            fake: Arc::new(Fake {
                closed_pulls: HashMap::from([(1, page(vec![], None))]),
                closed_issues: HashMap::from([(1, page(vec![], None))]),
                ..Default::default()
            }),
            ..bench
        };
        assert_eq!(bench.round().await, 0);
        assert_eq!(bench.calls(), vec!["closed pulls page=1", "closed issues page=1 since=2026-09-09T00:00:00Z"]);
        assert_eq!(meta(&bench.cache, "backfill:issues:a/b").as_deref(), Some("done:2026-09-09T00:00:00Z"));

        // A catch-up whose page ends in a pull request moves past it too.
        let mut with_pull = page(vec![closed_issue(3, "2026-09-10T00:00:00Z")], None);
        with_pull.newest = Some(at("2026-09-11T00:00:00Z"));
        let bench = Bench {
            fake: Arc::new(Fake {
                closed_pulls: HashMap::from([(1, page(vec![], None))]),
                closed_issues: HashMap::from([(1, with_pull)]),
                comments: comments([3]),
                ..Default::default()
            }),
            ..bench
        };
        assert_eq!(bench.round().await, 1);
        assert_eq!(meta(&bench.cache, "backfill:issues:a/b").as_deref(), Some("done:2026-09-11T00:00:00Z"));
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
        assert_eq!(Cursor::parse(None), Cursor::Page { page: 1, skip: 0 });
        assert_eq!(Cursor::parse(Some("7")), Cursor::Page { page: 7, skip: 0 });
        assert_eq!(Cursor::parse(Some("3+17")), Cursor::Page { page: 3, skip: 17 }, "stopped inside page three");
        assert_eq!(Cursor::parse(Some("done:2026-09-01T00:00:00Z")), Cursor::Done(at("2026-09-01T00:00:00Z")));
        assert_eq!(Cursor::parse(Some("done:soon")), Cursor::START);
        assert_eq!(Cursor::parse(Some("page three")), Cursor::START);
        assert_eq!(Cursor::parse(Some("3+some")), Cursor::START);
        assert_eq!(cursor_key(ItemKind::Issue, "SlateDB/SlateDB"), "backfill:issues:slatedb/slatedb");
    }
}
