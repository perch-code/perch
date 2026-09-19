//! The list pass.
//!
//! Every open pull request and issue of a declared repository is a row,
//! whether or not anyone was notified about it. A pass asks the two open
//! lists conditionally, upserts what they hold, and fetches once more any
//! item the cache still has open that the list no longer names, to learn
//! how it closed. The pass is a function over a `Source` and the cache
//! rather than over `Ctx`, so the harness can run one without the window
//! and the tests can run one against a fake GitHub.

use std::collections::HashSet;
use std::fmt;
use std::sync::Mutex;

use jiff::Timestamp;
use perch_github::{
    Allowance, Cache, CacheError, Discussion, GitHub, Issue, IssueComment, Item, ItemKind,
    ItemSubject, Listed, Page, PullRequest,
};

use super::Ctx;

/// What the pass and the backfill ask of GitHub, so a test can stand a
/// fake in for it. The closed lists, the discussion fetches and the
/// allowance are the backfill's; they are here so it paces itself by the
/// same source the pass reads.
pub(crate) trait Source {
    async fn list_open_pulls(
        &self,
        repo_full_name: &str,
        etag: Option<&str>,
    ) -> Result<Listed<PullRequest>, perch_github::Error>;
    async fn list_open_issues(
        &self,
        repo_full_name: &str,
        etag: Option<&str>,
    ) -> Result<Listed<Issue>, perch_github::Error>;
    async fn list_closed_pulls(
        &self,
        repo_full_name: &str,
        page: u32,
    ) -> Result<Page<PullRequest>, perch_github::Error>;
    async fn list_closed_issues(
        &self,
        repo_full_name: &str,
        page: u32,
        since: Option<Timestamp>,
    ) -> Result<Page<Issue>, perch_github::Error>;
    async fn fetch_pull_request(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<PullRequest, perch_github::Error>;
    async fn fetch_issue(&self, repo_full_name: &str, number: u64) -> Result<Issue, perch_github::Error>;
    async fn fetch_discussion(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Discussion, perch_github::Error>;
    async fn fetch_issue_comments(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Vec<IssueComment>, perch_github::Error>;
    fn allowance(&self) -> Option<Allowance>;
}

impl Source for GitHub {
    async fn list_open_pulls(
        &self,
        repo_full_name: &str,
        etag: Option<&str>,
    ) -> Result<Listed<PullRequest>, perch_github::Error> {
        GitHub::list_open_pulls(self, repo_full_name, etag).await
    }

    async fn list_open_issues(
        &self,
        repo_full_name: &str,
        etag: Option<&str>,
    ) -> Result<Listed<Issue>, perch_github::Error> {
        GitHub::list_open_issues(self, repo_full_name, etag).await
    }

    async fn list_closed_pulls(
        &self,
        repo_full_name: &str,
        page: u32,
    ) -> Result<Page<PullRequest>, perch_github::Error> {
        GitHub::list_closed_pulls(self, repo_full_name, page).await
    }

    async fn list_closed_issues(
        &self,
        repo_full_name: &str,
        page: u32,
        since: Option<Timestamp>,
    ) -> Result<Page<Issue>, perch_github::Error> {
        GitHub::list_closed_issues(self, repo_full_name, page, since).await
    }

    async fn fetch_pull_request(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<PullRequest, perch_github::Error> {
        GitHub::fetch_pull_request(self, repo_full_name, number).await
    }

    async fn fetch_issue(&self, repo_full_name: &str, number: u64) -> Result<Issue, perch_github::Error> {
        GitHub::fetch_issue(self, repo_full_name, number).await
    }

    async fn fetch_discussion(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Discussion, perch_github::Error> {
        GitHub::fetch_discussion(self, repo_full_name, number).await
    }

    async fn fetch_issue_comments(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Vec<IssueComment>, perch_github::Error> {
        GitHub::fetch_issue_comments(self, repo_full_name, number).await
    }

    fn allowance(&self) -> Option<Allowance> {
        GitHub::allowance(self)
    }
}

/// What stopped a pass or a backfill round: GitHub, the cache, having no
/// cache to write to, or the knowledge root.
#[derive(Debug)]
pub(crate) enum Error {
    GitHub(perch_github::Error),
    Cache(CacheError),
    /// No cache was opened, or a panic poisoned its lock.
    NoCache,
    /// A discussion could not be filed under the knowledge root.
    Knowledge(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::GitHub(e) => write!(f, "{e}"),
            Error::Cache(e) => write!(f, "cache: {e}"),
            Error::NoCache => write!(f, "there is no cache to write the items to"),
            Error::Knowledge(e) => write!(f, "knowledge: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<perch_github::Error> for Error {
    fn from(e: perch_github::Error) -> Self {
        Error::GitHub(e)
    }
}

impl From<CacheError> for Error {
    fn from(e: CacheError) -> Self {
        Error::Cache(e)
    }
}

/// What one pass did, as `owner/repo#n` labels, for the harness to print
/// and the poll loop to log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PassReport {
    /// Rows the pass made, and branch rows that took a pull request.
    pub added: Vec<String>,
    /// Rows whose subject moved.
    pub updated: Vec<String>,
    /// Rows whose subject fell out of the open list and was fetched
    /// closed or merged.
    pub closed: Vec<String>,
    /// Both lists answered 304, so no row was read or written.
    pub unchanged: bool,
}

impl PassReport {
    /// Whether no row changed, whatever the lists said.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.closed.is_empty()
    }
}

/// One list pass over a repository: pull requests, then issues.
///
/// Each list is asked with the tag stored from the last pass. A 304 means
/// nothing opened, closed or moved, and that list is done. A 200 is
/// upserted whole, then every item the cache holds open in the repository
/// that the list did not name is fetched by number and stored as GitHub
/// reports it: merged, closed, or now and then still open and missed by a
/// page boundary. The tag is written only after every upsert and closure
/// for that list succeeded, so a pass that failed part way asks
/// unconditionally next time and applies what this one did not. Rows the
/// pass made have no attention and no checkout: the reader declared the
/// repository and asked to see it, and that is all a row is.
///
/// The cache lock is taken for each stretch of cache work and never held
/// across a request, so a pass does not hold up a mark or an open while
/// GitHub answers.
pub(crate) async fn list_pass<S: Source>(
    source: &S,
    cache: &Mutex<Cache>,
    repo: &str,
    now: Timestamp,
) -> Result<PassReport, Error> {
    let mut report = PassReport::default();
    let pulls = pass_list(source, cache, repo, ItemKind::PullRequest, now, &mut report).await?;
    let issues = pass_list(source, cache, repo, ItemKind::Issue, now, &mut report).await?;
    report.unchanged = pulls == Answer::Unchanged && issues == Answer::Unchanged;
    Ok(report)
}

/// One pass over the app's own client and cache, and the sidebar told
/// once whatever happened: a failed pass has usually written rows before
/// it stopped, and they should show.
pub(super) async fn list_pass_for(ctx: &Ctx, repo: &str) -> Result<PassReport, Error> {
    let Some(cache) = ctx.cache.as_ref() else {
        return Err(Error::NoCache);
    };
    let outcome = list_pass(ctx.github.as_ref(), cache, repo, Timestamp::now()).await;
    if !matches!(&outcome, Ok(report) if report.is_empty()) {
        ctx.send_items();
    }
    outcome
}

/// How a list answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Unchanged,
    Changed,
}

/// One of the two lists, from the conditional request to the tag.
async fn pass_list<S: Source>(
    source: &S,
    cache: &Mutex<Cache>,
    repo: &str,
    kind: ItemKind,
    now: Timestamp,
    report: &mut PassReport,
) -> Result<Answer, Error> {
    let key = etag_key(kind, repo);
    let etag = with(cache, |c| c.meta(&key))?;
    let listed = match kind {
        ItemKind::PullRequest => {
            wrap(source.list_open_pulls(repo, etag.as_deref()).await?, ItemSubject::PullRequest)
        }
        ItemKind::Issue => wrap(source.list_open_issues(repo, etag.as_deref()).await?, ItemSubject::Issue),
    };
    let Listed::Changed { etag, items } = listed else {
        tracing::info!(repo, kind = kind.column(), "open list unchanged");
        return Ok(Answer::Unchanged);
    };
    tracing::info!(repo, kind = kind.column(), count = items.len(), "open list read");

    let mut named: HashSet<u64> = HashSet::new();
    // The rows are keyed by the repository as the payload names it, and a
    // repository since renamed or transferred answers the old name with
    // the new one in every entry. Both are looked in for what closed, and
    // the new name is remembered, so a list that came back empty because
    // the last of them closed still leads to the rows.
    let alias = alias_key(repo);
    let repos: Vec<String> = with(cache, |c| {
        let mut repos = vec![repo.to_string()];
        repos.extend(c.meta(&alias)?);
        for subject in items {
            named.insert(subject.number());
            let reported = subject.repo_full_name();
            if !repos.iter().any(|r| r.eq_ignore_ascii_case(reported)) {
                c.set_meta(&alias, reported)?;
                repos.push(reported.to_string());
            }
            upsert(c, repo, subject, now, report)?;
        }
        Ok(repos)
    })?;

    // What the cache still holds open that GitHub no longer lists. Read
    // after the upserts, so a row the list just made cannot be judged.
    let missing: Vec<(u64, String)> = with(cache, |c| {
        let mut missing = Vec::new();
        for r in &repos {
            missing.extend(
                c.open_subjects_in(r)?
                    .into_iter()
                    .filter(|(k, number, _)| *k == kind && !named.contains(number))
                    .map(|(_, number, id)| (number, id)),
            );
        }
        Ok(missing)
    })?;
    for (number, id) in missing {
        let fetched = match kind {
            ItemKind::PullRequest => source.fetch_pull_request(repo, number).await.map(ItemSubject::PullRequest),
            ItemKind::Issue => source.fetch_issue(repo, number).await.map(ItemSubject::Issue),
        };
        let subject = match fetched {
            // Deleted, or transferred where the token cannot follow: GitHub
            // will never answer for this number again, so asking again next
            // pass would fail the same way for good. The row closes.
            Err(perch_github::Error::Status { status: status @ (404 | 410), .. }) => {
                tracing::warn!(repo, number, status, "the open list no longer names it and GitHub has no such subject; stored closed");
                None
            }
            Err(e) => return Err(e.into()),
            // Transferred to another repository: the redirect was followed
            // and what came back is the subject at its new home, under a
            // new number. It is not this row's any more.
            Ok(subject) if subject.number() != number => {
                tracing::warn!(repo, number, renumbered = subject.number(), "the subject was transferred away; stored closed");
                None
            }
            Ok(subject) => Some(subject),
        };
        let open = subject.as_ref().is_some_and(ItemSubject::is_open);
        with(cache, |c| {
            let Some(mut item) = c.load_item(&id)? else { return Ok(()) };
            item.subject = match subject {
                Some(subject) => Some(subject),
                None => item.subject.take().map(closed),
            };
            c.update_item(&item)
        })?;
        tracing::info!(repo, number, open, "fetched a subject the open list no longer names");
        let label = format!("{repo}#{number}");
        if open {
            report.updated.push(label);
        } else {
            report.closed.push(label);
        }
    }

    // The tag last, and only now: written any earlier it would vouch for
    // rows this pass never wrote, and the next pass would take its word.
    with(cache, |c| match &etag {
        Some(etag) => c.set_meta(&key, etag),
        None => c.delete_meta(&key),
    })?;
    Ok(Answer::Changed)
}

/// One listed subject into the cache: the row that has it, else a checkout
/// of the reader's own on a pull request's head branch, else a new row.
/// The row is found by the repository the subject names, which is the
/// name it is stored under: a list asked under a name GitHub has since
/// redirected answers with the new name in every entry, and a find by the
/// declared name would miss the row and insert beside it.
fn upsert(
    cache: &Cache,
    repo: &str,
    subject: ItemSubject,
    now: Timestamp,
    report: &mut PassReport,
) -> Result<(), CacheError> {
    let number = subject.number();
    let label = format!("{repo}#{number}");

    if let Some(mut item) = cache.find_item_by_subject(subject.repo_full_name(), subject.kind(), number)? {
        let stored = item.subject.take();
        if !newer(subject.updated_at(), stored.as_ref().and_then(ItemSubject::updated_at)) {
            return Ok(());
        }
        item.subject = Some(match stored {
            Some(stored) => carry_forward(subject, &stored),
            None => subject,
        });
        cache.update_item(&item)?;
        report.updated.push(label);
        return Ok(());
    }

    if let ItemSubject::PullRequest(pr) = &subject {
        if let Some(mut item) = branch_item_for(cache, pr) {
            // The branch was cut against a base; the pull request says
            // which, and the review diffs against it from here on.
            if let Some(c) = item.checkout.as_mut() {
                c.base_ref = pr.base_ref.clone();
            }
            item.subject = Some(subject);
            cache.update_item(&item)?;
            tracing::info!(repo, number, item = %item.id, "a branch of the reader's own took its pull request");
            report.added.push(label);
            return Ok(());
        }
    }

    let mut item = Item::new(subject.repo_full_name(), now);
    item.subject = Some(subject);
    cache.insert_item(&item)?;
    report.added.push(label);
    Ok(())
}

/// A checkout of the reader's own on a pull request's head branch, with
/// no subject yet, that the pull request can attach to. The head
/// repository is the fork when there is one, and a branch of the same
/// name on the upstream is someone else's; a head with no repository is a
/// deleted fork, which is nobody's checkout. A root row the scan put away
/// because its branch is no longer checked out is passed over too: it
/// cannot be read, and a checkout of Perch's own can.
pub(crate) fn branch_item_for(cache: &Cache, pr: &PullRequest) -> Option<Item> {
    let head_repo = pr.head_repo_full_name.as_deref()?;
    cache
        .find_item_by_branch(head_repo, &pr.head_ref)
        .ok()
        .flatten()
        .filter(|i| i.subject.is_none())
        .filter(|i| !i.is_archived() || i.checkout.as_ref().is_some_and(|c| c.owned))
}

/// Whether an incoming subject is strictly newer than the stored one, so
/// an earlier page cannot write over a detail fetch that landed between
/// two pages, and a list that has not moved rewrites nothing. A subject
/// with no time is the oldest there is: a stored one with none gives way
/// to anything, and an incoming one with none replaces only its like.
fn newer(incoming: Option<Timestamp>, stored: Option<Timestamp>) -> bool {
    match (incoming, stored) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(incoming), Some(stored)) => incoming > stored,
    }
}

/// Whether a subject already in the row is strictly newer than one an
/// open or a refresh fetched, so a write that crossed a pass keeps what
/// the pass stored: an open fetches, then clones, and a pass that listed
/// the pull request while the clone ran has stored a later entry and a
/// tag that vouches for it. Written over, the older snapshot would stand
/// until the first page of the list moved for some other reason. Only
/// two dated subjects compare; anything else is written as fetched.
pub(super) fn stored_is_newer(stored: Option<Timestamp>, fetched: Option<Timestamp>) -> bool {
    matches!((stored, fetched), (Some(stored), Some(fetched)) if stored > fetched)
}

/// The stored subject marked closed, for one GitHub no longer answers
/// for. Nothing else about it is invented: the times stay what the last
/// answer said.
fn closed(mut subject: ItemSubject) -> ItemSubject {
    match &mut subject {
        ItemSubject::PullRequest(pr) => pr.state = "closed".into(),
        ItemSubject::Issue(issue) => issue.state = "closed".into(),
    }
    subject
}

/// A list entry carries no additions, deletions, changed files or merger,
/// because the list endpoint leaves them out. The stored subject's are
/// kept where the entry's are at their defaults, so a row does not lose
/// what a detail fetch gave it every time the list moves.
fn carry_forward(mut incoming: ItemSubject, stored: &ItemSubject) -> ItemSubject {
    if let (ItemSubject::PullRequest(pr), ItemSubject::PullRequest(old)) = (&mut incoming, stored) {
        if pr.additions == 0 && pr.deletions == 0 && pr.changed_files == 0 {
            pr.additions = old.additions;
            pr.deletions = old.deletions;
            pr.changed_files = old.changed_files;
        }
        if pr.merged_by.is_none() {
            pr.merged_by = old.merged_by.clone();
        }
    }
    incoming
}

/// The meta key a list's tag is kept under: `etag:pulls:<owner/repo>` or
/// `etag:issues:<owner/repo>`, the name lower-cased so a repository
/// spelled two ways in config is one key.
fn etag_key(kind: ItemKind, repo: &str) -> String {
    let list = match kind {
        ItemKind::PullRequest => "pulls",
        ItemKind::Issue => "issues",
    };
    format!("etag:{list}:{}", repo.to_lowercase())
}

/// The meta key that remembers the name GitHub answers a declared
/// repository under when it is not the declared one: `alias:<owner/repo>`,
/// the declared name lower-cased.
fn alias_key(repo: &str) -> String {
    format!("alias:{}", repo.to_lowercase())
}

/// A list of one subject type as a list of subjects, so the two lists go
/// through one upsert.
fn wrap<T>(listed: Listed<T>, into: impl Fn(T) -> ItemSubject) -> Listed<ItemSubject> {
    match listed {
        Listed::Unchanged => Listed::Unchanged,
        Listed::Changed { etag, items } => {
            Listed::Changed { etag, items: items.into_iter().map(into).collect() }
        }
    }
}

/// One stretch of cache work under the lock. The guard never crosses an
/// await, and a poisoned lock reads as no cache.
pub(super) fn with<T>(cache: &Mutex<Cache>, f: impl FnOnce(&Cache) -> Result<T, CacheError>) -> Result<T, Error> {
    let guard = cache.lock().map_err(|_| Error::NoCache)?;
    f(&guard).map_err(Error::Cache)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use perch_github::ItemCheckout;

    use super::super::fake::{
        at, cache, changed, issue, merged, meta, pull, store_issue, store_pull, Fake, REPO,
    };
    use super::*;

    fn pull_item(cache: &Mutex<Cache>, n: u64) -> Option<Item> {
        cache.lock().unwrap().find_item_by_subject(REPO, ItemKind::PullRequest, n).unwrap()
    }

    fn issue_item(cache: &Mutex<Cache>, n: u64) -> Option<Item> {
        cache.lock().unwrap().find_item_by_subject(REPO, ItemKind::Issue, n).unwrap()
    }

    const NOW: &str = "2026-09-18T12:00:00Z";

    #[tokio::test]
    async fn a_new_number_makes_a_row_with_no_attention_and_no_checkout() {
        let (_dir, cache) = cache();
        let fake = Fake {
            pulls: Some(changed(Some("W/\"p1\""), vec![pull(7, "2026-09-16T11:40:00Z")])),
            issues: Some(changed(Some("W/\"i1\""), vec![issue(42, "2026-09-16T11:40:00Z")])),
            ..Default::default()
        };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert_eq!(report.added, vec!["a/b#7", "a/b#42"]);
        assert!(report.updated.is_empty() && report.closed.is_empty());
        assert!(!report.unchanged);
        let item = pull_item(&cache, 7).expect("a row for the pull request");
        assert_eq!(item.attention_at, None, "a listed row is not attention");
        assert_eq!(item.checkout, None);
        assert_eq!(item.created_at, at(NOW));
        assert_eq!(item.last_opened_at, at(NOW));
        assert_eq!(item.repo_full_name, REPO);
        assert!(issue_item(&cache, 42).is_some());
        // The tags are stored once the rows are in.
        assert_eq!(meta(&cache, "etag:pulls:a/b").as_deref(), Some("W/\"p1\""));
        assert_eq!(meta(&cache, "etag:issues:a/b").as_deref(), Some("W/\"i1\""));
        // Asked with no tag the first time, and the closure step had
        // nothing to fetch.
        assert_eq!(fake.calls(), vec!["pulls etag=-", "issues etag=-"]);
    }

    #[tokio::test]
    async fn a_known_number_updates_its_subject() {
        let (_dir, cache) = cache();
        let stored = store_pull(&cache, pull(7, "2026-09-16T11:40:00Z"));
        let mut moved = pull(7, "2026-09-17T09:00:00Z");
        moved.title = "pull 7, renamed".into();
        let fake = Fake { pulls: Some(changed(None, vec![moved])), issues: Some(changed(None, vec![])), ..Default::default() };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert_eq!(report.updated, vec!["a/b#7"]);
        assert!(report.added.is_empty());
        let item = pull_item(&cache, 7).unwrap();
        assert_eq!(item.id, stored.id, "the same row");
        assert_eq!(item.title(), "pull 7, renamed");
        assert_eq!(item.pull_request().unwrap().updated_at, Some(at("2026-09-17T09:00:00Z")));
    }

    #[tokio::test]
    async fn an_older_or_equal_updated_at_does_not_replace_the_subject() {
        let (_dir, cache) = cache();
        let mut kept = pull(7, "2026-09-17T09:00:00Z");
        kept.title = "from a detail fetch".into();
        store_pull(&cache, kept);

        for stale in ["2026-09-17T09:00:00Z", "2026-09-16T11:40:00Z"] {
            let mut entry = pull(7, stale);
            entry.title = "from an earlier page".into();
            let fake = Fake { pulls: Some(changed(None, vec![entry])), issues: Some(changed(None, vec![])), ..Default::default() };
            let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();
            assert!(report.is_empty(), "{stale} is not newer");
            assert_eq!(pull_item(&cache, 7).unwrap().title(), "from a detail fetch");
        }
    }

    #[test]
    fn a_subject_with_no_time_gives_way_and_replaces_only_its_like() {
        assert!(newer(Some(at(NOW)), None), "anything beats no time");
        assert!(!newer(None, Some(at(NOW))), "no time beats nothing dated");
        assert!(newer(None, None), "two undated subjects: take the new one");
        assert!(!newer(Some(at(NOW)), Some(at(NOW))), "equal is not newer");
    }

    #[tokio::test]
    async fn a_list_entry_with_zero_stats_keeps_the_stored_stats() {
        let (_dir, cache) = cache();
        let mut detailed = pull(7, "2026-09-16T11:40:00Z");
        detailed.additions = 412;
        detailed.deletions = 87;
        detailed.changed_files = 9;
        store_pull(&cache, detailed);
        let fake = Fake {
            pulls: Some(changed(None, vec![pull(7, "2026-09-17T09:00:00Z")])),
            issues: Some(changed(None, vec![])),
            ..Default::default()
        };

        list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        let pr = pull_item(&cache, 7).unwrap().pull_request().unwrap().clone();
        assert_eq!((pr.additions, pr.deletions, pr.changed_files), (412, 87, 9));
        assert_eq!(pr.updated_at, Some(at("2026-09-17T09:00:00Z")), "the rest of the entry landed");
    }

    #[tokio::test]
    async fn a_branch_item_on_the_head_branch_takes_the_subject_and_keeps_its_id() {
        let (_dir, cache) = cache();
        let mut branch = Item::new(REPO, at("2026-09-10T00:00:00Z"));
        branch.checkout = Some(ItemCheckout {
            branch: "branch-7".into(),
            base_ref: "develop".into(),
            head_sha: "h7".into(),
            path: "/x/b".into(),
            owned: false,
            on_disk: true,
            churn: Some((3, 1)),
            last_seen_head: None,
        });
        cache.lock().unwrap().insert_item(&branch).unwrap();
        let fake = Fake {
            pulls: Some(changed(None, vec![pull(7, "2026-09-16T11:40:00Z")])),
            issues: Some(changed(None, vec![])),
            ..Default::default()
        };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert_eq!(report.added, vec!["a/b#7"]);
        let item = pull_item(&cache, 7).expect("the branch row has the subject");
        assert_eq!(item.id, branch.id);
        let checkout = item.checkout.expect("the checkout stays");
        assert_eq!(checkout.branch, "branch-7");
        assert_eq!(checkout.base_ref, "main", "the pull request's base");
        assert_eq!(cache.lock().unwrap().load_items().unwrap().len(), 1, "no second row");
    }

    #[tokio::test]
    async fn a_number_missing_from_the_list_is_fetched_and_stored_closed() {
        let (_dir, cache) = cache();
        store_pull(&cache, pull(7, "2026-09-16T11:40:00Z"));
        let listed = store_pull(&cache, pull(8, "2026-09-16T11:40:00Z"));
        let closed = store_issue(&cache, issue(42, "2026-09-16T11:40:00Z"));
        let mut done = issue(42, "2026-09-18T08:00:00Z");
        done.state = "closed".into();
        done.state_reason = Some("completed".into());
        let fake = Fake {
            pulls: Some(changed(Some("W/\"p2\""), vec![pull(8, "2026-09-16T11:40:00Z")])),
            issues: Some(changed(Some("W/\"i2\""), vec![])),
            pulls_by_number: HashMap::from([(7, merged(7, "2026-09-18T08:00:00Z"))]),
            issues_by_number: HashMap::from([(42, done)]),
            ..Default::default()
        };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert_eq!(report.closed, vec!["a/b#7", "a/b#42"]);
        assert!(report.added.is_empty() && report.updated.is_empty());
        let pr = pull_item(&cache, 7).unwrap().pull_request().unwrap().clone();
        assert!(pr.merged && !pr.is_open());
        assert_eq!(pr.merged_by.as_deref(), Some("rohan"), "stored as GitHub reports it");
        let issue = issue_item(&cache, 42).unwrap();
        assert_eq!(issue.id, closed.id);
        assert!(!issue.issue().unwrap().is_open());
        assert_eq!(pull_item(&cache, 8).unwrap().id, listed.id, "the listed one is left as it was");
        assert_eq!(
            fake.calls(),
            vec!["pulls etag=-", "pull 7", "issues etag=-", "issue 42"],
            "one fetch per missing number, and no discussion"
        );
        assert_eq!(meta(&cache, "etag:pulls:a/b").as_deref(), Some("W/\"p2\""));
        assert_eq!(meta(&cache, "etag:issues:a/b").as_deref(), Some("W/\"i2\""));
    }

    #[tokio::test]
    async fn a_failure_part_way_leaves_the_tag_as_it_was() {
        let (_dir, cache) = cache();
        cache.lock().unwrap().set_meta("etag:pulls:a/b", "W/\"old\"").unwrap();
        store_pull(&cache, pull(7, "2026-09-16T11:40:00Z"));
        // The list moved on and no longer names 7; the fetch to learn why
        // fails, so the pass stops before the tag.
        let fake = Fake {
            pulls: Some(changed(Some("W/\"new\""), vec![pull(9, "2026-09-17T09:00:00Z")])),
            issues: Some(changed(None, vec![])),
            ..Default::default()
        };

        let err = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap_err();

        assert!(matches!(err, Error::GitHub(_)), "{err}");
        assert_eq!(meta(&cache, "etag:pulls:a/b").as_deref(), Some("W/\"old\""), "the old tag stands");
        assert!(pull_item(&cache, 9).is_some(), "what was upserted before the failure stays");
        assert!(pull_item(&cache, 7).unwrap().pull_request().unwrap().is_open(), "nothing was invented for 7");
        assert_eq!(fake.calls(), vec!["pulls etag=W/\"old\"", "pull 7"], "the issues list was never reached");
    }

    #[tokio::test]
    async fn a_failed_issues_list_leaves_the_pulls_tag_written_and_the_issues_tag_alone() {
        let (_dir, cache) = cache();
        cache.lock().unwrap().set_meta("etag:issues:a/b", "W/\"old\"").unwrap();
        let fake = Fake {
            pulls: Some(changed(Some("W/\"p\""), vec![pull(7, "2026-09-16T11:40:00Z")])),
            issues: None,
            ..Default::default()
        };

        let err = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap_err();

        assert!(matches!(err, Error::GitHub(_)));
        assert_eq!(meta(&cache, "etag:pulls:a/b").as_deref(), Some("W/\"p\""), "that list finished");
        assert_eq!(meta(&cache, "etag:issues:a/b").as_deref(), Some("W/\"old\""), "that one did not");
    }

    #[tokio::test]
    async fn a_poisoned_cache_stops_the_pass_before_anything_is_asked() {
        let (_dir, cache) = cache();
        cache.lock().unwrap().set_meta("etag:pulls:a/b", "W/\"old\"").unwrap();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = cache.lock().unwrap();
            panic!("a task died holding the cache");
        }));
        let fake = Fake { pulls: Some(changed(None, vec![pull(7, NOW)])), ..Default::default() };

        let err = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap_err();

        assert!(matches!(err, Error::NoCache), "{err}");
        assert!(fake.calls().is_empty(), "no request without a cache to write to");
        let inner = cache.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(inner.meta("etag:pulls:a/b").unwrap().as_deref(), Some("W/\"old\""));
    }

    #[tokio::test]
    async fn unchanged_touches_nothing() {
        let (_dir, cache) = cache();
        cache.lock().unwrap().set_meta("etag:pulls:a/b", "W/\"p\"").unwrap();
        cache.lock().unwrap().set_meta("etag:issues:a/b", "W/\"i\"").unwrap();
        let stored = store_pull(&cache, pull(7, "2026-09-16T11:40:00Z"));
        let fake = Fake { pulls: Some(Listed::Unchanged), issues: Some(Listed::Unchanged), ..Default::default() };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert!(report.unchanged);
        assert!(report.is_empty());
        assert_eq!(fake.calls(), vec!["pulls etag=W/\"p\"", "issues etag=W/\"i\""], "asked with the stored tags");
        assert_eq!(cache.lock().unwrap().load_items().unwrap(), vec![stored]);
        assert_eq!(meta(&cache, "etag:pulls:a/b").as_deref(), Some("W/\"p\""));
        assert_eq!(meta(&cache, "etag:issues:a/b").as_deref(), Some("W/\"i\""));
    }

    #[tokio::test]
    async fn one_list_unchanged_and_the_other_not_is_not_unchanged() {
        let (_dir, cache) = cache();
        let fake = Fake {
            pulls: Some(Listed::Unchanged),
            issues: Some(changed(None, vec![issue(42, NOW)])),
            ..Default::default()
        };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert!(!report.unchanged);
        assert_eq!(report.added, vec!["a/b#42"]);
    }

    #[tokio::test]
    async fn a_200_without_a_tag_deletes_the_stored_key() {
        let (_dir, cache) = cache();
        cache.lock().unwrap().set_meta("etag:pulls:a/b", "W/\"p\"").unwrap();
        let fake = Fake {
            pulls: Some(changed(None, vec![pull(7, NOW)])),
            issues: Some(changed(None, vec![])),
            ..Default::default()
        };

        list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert_eq!(meta(&cache, "etag:pulls:a/b"), None, "the next pass asks unconditionally");
        assert_eq!(fake.calls()[0], "pulls etag=W/\"p\"", "this one asked with the old tag");
    }

    #[test]
    fn the_tag_key_lower_cases_the_repository() {
        assert_eq!(etag_key(ItemKind::PullRequest, "SlateDB/SlateDB"), "etag:pulls:slatedb/slatedb");
        assert_eq!(etag_key(ItemKind::Issue, "a/b"), "etag:issues:a/b");
    }

    #[tokio::test]
    async fn a_missing_number_still_open_on_fetch_is_an_update_not_a_closure() {
        let (_dir, cache) = cache();
        store_pull(&cache, pull(7, "2026-09-16T11:40:00Z"));
        // Missed by a page boundary: GitHub still says open when asked.
        let fake = Fake {
            pulls: Some(changed(None, vec![])),
            issues: Some(changed(None, vec![])),
            pulls_by_number: HashMap::from([(7, pull(7, "2026-09-17T09:00:00Z"))]),
            ..Default::default()
        };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert_eq!(report.updated, vec!["a/b#7"]);
        assert!(report.closed.is_empty());
        assert!(pull_item(&cache, 7).unwrap().pull_request().unwrap().is_open());
    }

    /// A pull request as a repository since renamed lists it: asked under
    /// `a/b`, every entry names `c/d`.
    fn pull_moved(n: u64, updated: &str) -> PullRequest {
        let mut pr = pull(n, updated);
        pr.repo_full_name = "c/d".into();
        pr.head_repo_full_name = Some("c/d".into());
        pr
    }

    #[tokio::test]
    async fn a_repository_that_answers_under_another_name_is_one_row_found_and_closed_by_that_name() {
        let (_dir, cache) = cache();
        let fake = Fake {
            pulls: Some(changed(Some("W/\"p1\""), vec![pull_moved(7, "2026-09-16T11:40:00Z")])),
            issues: Some(changed(None, vec![])),
            ..Default::default()
        };
        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();
        assert_eq!(report.added, vec!["a/b#7"]);
        let stored = cache.lock().unwrap().find_item_by_subject("c/d", ItemKind::PullRequest, 7).unwrap();
        let stored = stored.expect("keyed by the name the payload gives");

        // The next 200 finds the row where it is rather than inserting
        // beside it and failing on the subject index for good.
        let fake = Fake {
            pulls: Some(changed(Some("W/\"p2\""), vec![pull_moved(7, "2026-09-17T09:00:00Z")])),
            issues: Some(changed(None, vec![])),
            ..Default::default()
        };
        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();
        assert_eq!(report.updated, vec!["a/b#7"]);
        assert_eq!(cache.lock().unwrap().load_items().unwrap().len(), 1, "one row, not two");
        assert_eq!(meta(&cache, "etag:pulls:a/b").as_deref(), Some("W/\"p2\""), "the tag moved on");

        // And when it merges, and the list comes back empty, the closure
        // step still looks under the name the row is keyed by, which the
        // earlier lists taught it.
        assert_eq!(meta(&cache, "alias:a/b").as_deref(), Some("c/d"));
        let mut done = merged(7, "2026-09-18T08:00:00Z");
        done.repo_full_name = "c/d".into();
        let fake = Fake {
            pulls: Some(changed(Some("W/\"p3\""), vec![])),
            issues: Some(changed(None, vec![])),
            pulls_by_number: HashMap::from([(7, done)]),
            ..Default::default()
        };
        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();
        assert_eq!(report.closed, vec!["a/b#7"]);
        assert_eq!(fake.calls(), vec!["pulls etag=W/\"p2\"", "pull 7", "issues etag=-"]);
        let item = cache.lock().unwrap().load_item(&stored.id).unwrap().unwrap();
        assert!(item.pull_request().unwrap().merged, "stored as GitHub reports it");
    }

    #[tokio::test]
    async fn a_number_github_no_longer_answers_for_is_stored_closed_and_the_pass_goes_on() {
        let (_dir, cache) = cache();
        cache.lock().unwrap().set_meta("etag:issues:a/b", "W/\"old\"").unwrap();
        // Deleted by a maintainer: 410 from now until the end of time. A
        // pass that failed on it would fail the same way every poll, and
        // the tag would never move.
        let deleted = store_issue(&cache, issue(42, "2026-09-16T11:40:00Z"));
        let gone = store_pull(&cache, pull(7, "2026-09-16T11:40:00Z"));
        // Closed after the deleted one, by number, which the pass must
        // still reach.
        let closed_after = store_issue(&cache, issue(43, "2026-09-16T11:40:00Z"));
        let mut done = issue(43, "2026-09-18T08:00:00Z");
        done.state = "closed".into();
        let fake = Fake {
            pulls: Some(changed(Some("W/\"p\""), vec![])),
            issues: Some(changed(Some("W/\"i\""), vec![])),
            issues_by_number: HashMap::from([(43, done)]),
            refused: HashMap::from([(42, 410), (7, 404)]),
            ..Default::default()
        };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert_eq!(report.closed, vec!["a/b#7", "a/b#42", "a/b#43"]);
        assert_eq!(fake.calls(), vec!["pulls etag=-", "pull 7", "issues etag=W/\"old\"", "issue 42", "issue 43"]);
        let item = cache.lock().unwrap().load_item(&deleted.id).unwrap().unwrap();
        let stored = item.issue().unwrap();
        assert!(!stored.is_open(), "closed, so the next pass does not ask again");
        assert_eq!(stored.title, "issue 42", "the rest of the snapshot is as it was");
        assert!(!cache.lock().unwrap().load_item(&gone.id).unwrap().unwrap().pull_request().unwrap().is_open());
        assert!(!cache.lock().unwrap().load_item(&closed_after.id).unwrap().unwrap().issue().unwrap().is_open());
        assert_eq!(meta(&cache, "etag:issues:a/b").as_deref(), Some("W/\"i\""), "the pass finished, so the tag is written");
        assert!(cache.lock().unwrap().open_subjects_in(REPO).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_subject_transferred_away_answers_under_a_new_number_and_is_stored_closed_here() {
        let (_dir, cache) = cache();
        let moved = store_issue(&cache, issue(5, "2026-09-16T11:40:00Z"));
        // The redirect was followed to the issue's new home, where it is
        // #9 and still open. It is not a/b#5 any more.
        let fake = Fake {
            pulls: Some(changed(None, vec![])),
            issues: Some(changed(None, vec![])),
            issues_by_number: HashMap::from([(5, issue(9, "2026-09-18T08:00:00Z"))]),
            ..Default::default()
        };

        let report = list_pass(&fake, &cache, REPO, at(NOW)).await.unwrap();

        assert_eq!(report.closed, vec!["a/b#5"]);
        let item = cache.lock().unwrap().load_item(&moved.id).unwrap().unwrap();
        let stored = item.issue().unwrap();
        assert_eq!(stored.number, 5, "the row keeps its own number");
        assert!(!stored.is_open());
        assert!(issue_item(&cache, 9).is_none(), "the other repository's issue is not filed under this one");
    }

    #[test]
    fn a_stored_subject_is_kept_only_when_it_is_dated_and_strictly_newer() {
        let earlier = Some(at("2026-09-16T11:40:00Z"));
        let later = Some(at("2026-09-17T09:00:00Z"));
        assert!(stored_is_newer(later, earlier), "a pass stored a later entry while the open cloned");
        assert!(!stored_is_newer(earlier, later), "what was fetched is the newer");
        assert!(!stored_is_newer(later, later), "equal writes: the detail carries what the list omits");
        assert!(!stored_is_newer(None, later), "a row with no subject takes anything");
        assert!(!stored_is_newer(later, None), "an undated fetch is still written");
    }
}
