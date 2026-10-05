//! GitHub REST client: notification history and targeted item lookups.
//!
//! Uses octocrab's raw `_get_with_headers` rather than its typed builders,
//! because polling needs `X-Poll-Interval` and `Last-Modified`, every
//! other read asks with the `ETag` it last saw, and rate-limit headers
//! classify failures.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{
    HeaderMap, HeaderValue, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, LINK,
};
use http::StatusCode;
use http_body_util::BodyExt;
use jiff::Timestamp;
use octocrab::Octocrab;

use crate::commits::{
    commit_variables, CommitOnBranchData, FileTextData, Unreadable, COMMIT_ON_BRANCH_MUTATION,
    FILE_TEXT_QUERY,
};
use crate::discussion::{
    resolve_thread_mutation, CommentPlace, Discussion, IssueComment, IssueCommentPayload, PullReviewEvent,
    PullSummary, ResolveThreadData, Review, ReviewComment, ReviewCommentDraft,
    ReviewCommentPayload, ReviewPayload, ReviewThreadState, ReviewThreadsData,
    REVIEW_THREADS_MAX_PAGES, REVIEW_THREADS_QUERY,
};
use crate::items::{Issue, IssueCloseReason, IssuePayload};
use crate::model::{SubjectSnapshot, SubjectState, Thread};
use crate::pulls::{
    check_sighting, rest_check_sighting, CheckSighting, CommitMessage, MergeMethod, MergeRequest, MergeResult,
    PullPayload, PullRequest, PullRequestOverview, PullRequestOverviewAccumulator,
    PullRequestOverviewQuery, PULL_REQUEST_OVERVIEW_QUERY,
};
use crate::releases::{Release, ReleaseAsset};
use crate::reactions::{
    reaction_mutation, reactions_query, NodeReading, ReactionKind, ReactionMutationData, Reactions,
    ReactionsData, NODES_PER_QUERY,
};
use crate::responses::{Known, Responses, BUDGET_BYTES};
use crate::subjects::{SubjectLook, SubjectLookData};
use crate::timeline::{
    fill_logins, same_moment, tidy, wants_login, DeletedAuthorsData, PullCommitPayload, PullEvent,
    PullEventKind, TimelineEventPayload, DELETED_AUTHORS_QUERY, TIMELINE_MAX_PAGES,
};
use crate::workflow_runs::{Checks, WorkflowRunPayload, WorkflowRunsPayload};

/// Page size for the notifications list. GitHub caps `per_page` at 50 here.
const PER_PAGE: usize = 50;
/// Fallback when the server sends no `X-Poll-Interval`.
const DEFAULT_POLL_SECS: u64 = 60;
/// Page size for discussion timelines. GitHub allows a hundred there.
const LIST_PER_PAGE: usize = 100;
/// Hard cap on pages for discussion lists.
const LIST_MAX_PAGES: usize = 100;
/// API requests in flight at once, across every clone. One at a time left
/// an open waiting behind whatever the pollers had queued; GitHub asks
/// for few concurrent requests, not none.
const API_CONCURRENCY: usize = 4;
/// The log target for each request's rate-limit cost. Off unless asked
/// for: `RUST_LOG=perch=info,perch_github::spend=debug,warn`.
const SPEND: &str = "perch_github::spend";
/// How long a pull request read stands for the discussion asked for after
/// it.
const JUST_READ: Duration = Duration::from_secs(5);
/// Asked beside every GraphQL query while the spend log is on, so the
/// answer says what the query cost.
const COST_FIELD: &str = "perch_cost: rateLimit { cost }";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GitHub rejected the token")]
    Unauthorized,
    #[error("rate limited")]
    RateLimited {
        reset_epoch: Option<i64>,
        retry_after_secs: Option<u64>,
    },
    #[error("GitHub returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("could not parse GitHub's response: {0}")]
    Decode(String),
    #[error("GitHub GraphQL returned errors: {0}")]
    Graphql(String),
}

/// What asking after a branch's pull request gave back.
#[derive(Debug, Clone, PartialEq)]
pub enum PullLookup {
    /// A 304: whatever was known still holds.
    Unchanged,
    /// A fresh answer, and the tag to ask with next time.
    Answer {
        pull: Option<Box<PullRequest>>,
        etag: Option<String>,
    },
}

/// The tags a commit's check runs and statuses were last read with. A
/// change in either means the rollup may have moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckTags {
    pub runs: Option<String>,
    pub statuses: Option<String>,
}

/// What a probe of a commit's checks found: the tags to ask with next,
/// and, when both listings were read whole, each check in them.
#[derive(Debug, Clone)]
pub struct CheckProbe {
    pub tags: CheckTags,
    seen: Option<CheckSighting>,
}

impl CheckProbe {
    /// Whether the listings read say what `checks` says of every check:
    /// the same checks, each in the same state. Nothing is known to agree
    /// when either listing came back unchanged or was cut at a page.
    pub fn agrees_with(&self, checks: &Checks) -> bool {
        self.seen
            .as_ref()
            .is_some_and(|seen| *seen == check_sighting(checks))
    }
}

/// What GitHub kept of an edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditedComment {
    pub body: String,
    /// For a description, when its pull request or issue last changed by
    /// GitHub's clock, which the edit moved. None for anything else, or
    /// when GitHub did not say.
    pub updated_at: Option<Timestamp>,
}

/// Outcome of a notifications poll.
#[derive(Debug, Clone, PartialEq)]
pub struct ListResult {
    /// The complete API result when `unchanged` is false. Empty when the
    /// conditional probe returned 304 and the cached result still stands.
    pub threads: Vec<Thread>,
    pub unchanged: bool,
    /// The validator to use for the next poll.
    pub last_modified: Option<String>,
    pub poll_interval: Duration,
}

/// One raw `/notifications` page response, before its body is decoded.
struct NotificationPage {
    status: StatusCode,
    last_modified: Option<String>,
    poll_interval: Duration,
    body: String,
}

/// One response with its headers read before the body is consumed.
struct Fetched {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

#[derive(Clone)]
pub struct GitHub {
    octo: Octocrab,
    http: reqwest::Client,
    /// Bounds API requests across clones. Each is checked against the
    /// rate-limit window once it holds a permit, so a response that closes
    /// the window gates every request that has not been sent yet.
    api_requests: Arc<tokio::sync::Semaphore>,
    /// Shared by every clone so one rate-limit response gates all later API
    /// calls, including item fetches spawned outside the poll loop.
    rate_limit_until: Arc<AtomicI64>,
    /// The search API's own window, thirty requests a minute. A search
    /// that spends it gates the next search, not the rest of the API.
    search_limit_until: Arc<AtomicI64>,
    /// The bodies of recent reads and their tags, shared by every clone,
    /// so a read of an unchanged path costs a 304.
    responses: Arc<std::sync::Mutex<Responses>>,
    /// What this process has spent of each rate-limit resource, for the
    /// spend log. Shared by every clone.
    spent: Arc<std::sync::Mutex<HashMap<String, Spent>>>,
    /// The token, for a picture uploaded to github.com: Octocrab holds
    /// its own copy and sends it to the API only.
    token: Arc<str>,
}

/// One rate-limit resource's spend: in each window by the epoch GitHub
/// says it resets at, and since launch. GitHub can answer from more than
/// one window at once, each with its own `used` and reset, so a window is
/// counted by its reset rather than by the one seen last.
#[derive(Debug, Default)]
struct Spent {
    windows: std::collections::BTreeMap<i64, i64>,
    total: i64,
}

impl GitHub {
    pub fn new(token: &str) -> Result<Self, Error> {
        Self::build(token, None)
    }

    /// Point the client at a different API root. Used by tests to stand up a
    /// local server in place of api.github.com.
    pub fn with_base_uri(token: &str, base_uri: &str) -> Result<Self, Error> {
        Self::build(token, Some(base_uri))
    }

    fn build(token: &str, base_uri: Option<&str>) -> Result<Self, Error> {
        // Octocrab already pins the GitHub REST API version. Adding the same
        // header here produces two values, which GitHub joins with a comma and
        // rejects as an unsupported version.
        let mut builder = Octocrab::builder().personal_token(token.to_string());
        if let Some(base) = base_uri {
            builder = builder
                .base_uri(base)
                .map_err(|e| Error::Network(e.to_string()))?;
        }
        let octo = builder.build().map_err(|e| Error::Network(e.to_string()))?;

        let http = reqwest::Client::builder()
            .user_agent(concat!("perch/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;

        Ok(Self {
            octo,
            http,
            api_requests: Arc::new(tokio::sync::Semaphore::new(API_CONCURRENCY)),
            rate_limit_until: Arc::new(AtomicI64::new(0)),
            search_limit_until: Arc::new(AtomicI64::new(0)),
            responses: Arc::new(std::sync::Mutex::new(Responses::new(BUDGET_BYTES))),
            spent: Arc::default(),
            token: Arc::from(token),
        })
    }

    /// Fetch the complete notifications result unconditionally.
    ///
    /// This wrapper is useful for callers without a cached validator. Polling
    /// callers should use [`Self::list_threads_since`].
    pub async fn list_threads(&self) -> Result<ListResult, Error> {
        self.list_threads_since(None).await
    }

    /// Fetch the authenticated user's GitHub notifications list.
    ///
    /// `all=true` keeps read rows alongside unread ones, but can also include
    /// Done history because the payload has no Inbox/Done membership field.
    /// `last_modified` is used only for a conditional request. A 304 keeps the
    /// cached result. A 200 is not the inbox: GitHub answers it with only the
    /// threads updated since `If-Modified-Since`, so the list is fetched again
    /// without the validator.
    pub async fn list_threads_since(
        &self,
        last_modified: Option<&str>,
    ) -> Result<ListResult, Error> {
        let mut probe_interval = None;
        if let Some(last_modified) = last_modified {
            let probe = self.fetch_notification_page(1, Some(last_modified)).await?;
            if probe.status == StatusCode::NOT_MODIFIED {
                return Ok(ListResult {
                    threads: Vec::new(),
                    unchanged: true,
                    last_modified: probe
                        .last_modified
                        .or_else(|| Some(last_modified.to_string())),
                    poll_interval: probe.poll_interval,
                });
            }
            probe_interval = Some(probe.poll_interval);
        }

        let mut threads = Vec::new();
        let mut page = 1usize;
        let mut response_last_modified = None;
        let mut poll_interval = Duration::from_secs(DEFAULT_POLL_SECS);

        loop {
            let response = self.fetch_notification_page(page, None).await?;
            if page == 1 {
                response_last_modified = response.last_modified.clone();
                poll_interval = probe_interval.map_or(response.poll_interval, |probe| {
                    probe.max(response.poll_interval)
                });
            } else {
                poll_interval = poll_interval.max(response.poll_interval);
            }

            let page_threads: Vec<Thread> =
                serde_json::from_str(&response.body).map_err(|e| Error::Decode(e.to_string()))?;
            let is_last_page = page_threads.len() < PER_PAGE;
            threads.extend(page_threads);
            if is_last_page {
                break;
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| Error::Decode("notification page number overflowed".into()))?;
        }

        Ok(ListResult {
            threads,
            unchanged: false,
            last_modified: response_last_modified,
            poll_interval,
        })
    }

    /// Mark one notification read, the same operation as opening it in
    /// github.com's notifications inbox.
    pub async fn mark_thread_read(&self, thread_id: &str) -> Result<(), Error> {
        let path = notification_thread_path(thread_id, "")?;
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._patch(&path, None::<&()>)
            .await
            .map_err(map_octocrab_error)?;
        self.finish_mutation(
            response,
            &[StatusCode::RESET_CONTENT, StatusCode::NOT_MODIFIED],
            &format!("PATCH {path}"),
        )
        .await
    }

    /// Remove one notification from github.com's inbox as Done.
    pub async fn mark_thread_done(&self, thread_id: &str) -> Result<(), Error> {
        let path = notification_thread_path(thread_id, "")?;
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._delete(&path, None::<&()>)
            .await
            .map_err(map_octocrab_error)?;
        self.finish_mutation(response, &[StatusCode::NO_CONTENT], &format!("DELETE {path}"))
            .await
    }

    /// Ignore future activity on a notification thread, including when the
    /// user watches its repository.
    pub async fn ignore_thread(&self, thread_id: &str) -> Result<(), Error> {
        let path = notification_thread_path(thread_id, "/subscription")?;
        let body = serde_json::json!({ "ignored": true });
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._put(&path, Some(&body))
            .await
            .map_err(map_octocrab_error)?;
        self.finish_mutation(
            response,
            &[StatusCode::OK, StatusCode::NOT_MODIFIED],
            &format!("PUT {path}"),
        )
            .await
    }

    /// One page of `/notifications`, including the headers that pace and
    /// validate polling.
    async fn fetch_notification_page(
        &self,
        page: usize,
        last_modified: Option<&str>,
    ) -> Result<NotificationPage, Error> {
        let uri =
            format!("/notifications?all=true&participating=false&per_page={PER_PAGE}&page={page}");
        let fetched = self.get(&uri, modified_since(last_modified)).await?;
        let last_modified = header_str(&fetched.headers, LAST_MODIFIED.as_str());
        let poll_interval = header_str(&fetched.headers, "x-poll-interval")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|seconds| Duration::from_secs(seconds.max(1)))
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_POLL_SECS));

        Ok(NotificationPage {
            status: fetched.status,
            last_modified,
            poll_interval,
            body: fetched.body,
        })
    }

    /// Fetch a pull request by number.
    ///
    /// `repo_full_name` is the repository it was asked for under, used only
    /// when the payload omits its own base repository.
    pub async fn fetch_pull_request(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<PullRequest, Error> {
        let body = self
            .get_text(&format!("/repos/{repo_full_name}/pulls/{number}"))
            .await?;
        let payload: PullPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;

        Ok(payload.into_pull_request(repo_full_name))
    }

    /// GitHub's computed merge state and complete check/status rollup for a
    /// pull request. Connections larger than GitHub's hundred-row page are
    /// drained, with every page fenced to the first page's head SHA.
    pub async fn fetch_pull_request_overview(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<PullRequestOverview, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        let number = i32::try_from(number)
            .map_err(|_| Error::Decode(format!("pull request number is too large: {number}")))?;
        let mut cursor = None::<String>;
        let mut seen_cursors = std::collections::HashSet::<String>::new();
        let mut overview = None::<PullRequestOverviewAccumulator>;

        loop {
            let payload = serde_json::json!({
                "query": PULL_REQUEST_OVERVIEW_QUERY,
                "variables": {
                    "owner": owner,
                    "name": name,
                    "number": number,
                    "contextsCursor": cursor.as_deref(),
                }
            });
            let data: PullRequestOverviewQuery = self
                .graphql(&payload, "pull request overview")
                .await?;
            let page = data.into_page().ok_or_else(|| {
                Error::Decode(format!(
                    "GitHub returned no pull request for {repo_full_name}#{number}"
                ))
            })?;
            match overview.as_mut() {
                Some(overview) => overview.push(page).map_err(Error::Decode)?,
                None => {
                    overview = Some(
                        PullRequestOverviewAccumulator::new(page).map_err(Error::Decode)?,
                    );
                }
            }

            let next = overview
                .as_ref()
                .and_then(|overview| overview.next_contexts_cursor())
                .map(str::to_owned);
            let Some(next) = next else { break };
            if !seen_cursors.insert(next.clone()) {
                return Err(Error::Decode(
                    "GitHub repeated a check-context page cursor".into(),
                ));
            }
            cursor = Some(next);
        }

        overview
            .map(PullRequestOverviewAccumulator::into_overview)
            .ok_or_else(|| Error::Decode("GitHub returned no pull request overview".into()))
    }

    /// Merge exactly the head the user confirmed. GitHub rejects the request
    /// with 409 if the pull request moved before this reaches the server.
    /// A merge forgets the kept read of the pull request, as a close does.
    pub async fn merge_pull_request(
        &self,
        repo_full_name: &str,
        number: u64,
        expected_head_sha: &str,
        method: MergeMethod,
        message: Option<&CommitMessage>,
    ) -> Result<MergeResult, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        if expected_head_sha.is_empty() || expected_head_sha.trim() != expected_head_sha {
            return Err(Error::Decode(
                "expected pull request head SHA is empty or invalid".into(),
            ));
        }
        let path = format!("/repos/{owner}/{name}/pulls/{number}/merge");
        let body = MergeRequest {
            sha: expected_head_sha,
            merge_method: method.as_str(),
            commit_title: message.map(|message| message.title.as_str()),
            commit_message: message.map(|message| message.body.as_str()),
        };
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._put(&path, Some(&body))
            .await
            .map_err(map_octocrab_error)?;
        let body = self.finish_response(response, &[StatusCode::OK], &format!("PUT {path}")).await?;
        self.forget_pull_request(repo_full_name, number);
        serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))
    }

    /// Close a pull request without merging it: `PATCH /pulls/{number}`
    /// with `state: closed`, which answers with the pull request as it
    /// now stands. The kept read of it is forgotten, so the discussion
    /// read after the close asks GitHub rather than taking a read from
    /// moments before it as current.
    pub async fn close_pull_request(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<PullRequest, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;

        #[derive(serde::Serialize)]
        struct Request {
            state: &'static str,
        }

        let path = format!("/repos/{owner}/{name}/pulls/{number}");
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._patch(&path, Some(&Request { state: "closed" }))
            .await
            .map_err(map_octocrab_error)?;
        let body = self
            .finish_response(response, &[StatusCode::OK], &format!("PATCH {path}"))
            .await?;
        self.forget_pull_request(repo_full_name, number);
        let payload: PullPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(payload.into_pull_request(repo_full_name))
    }

    /// Convert a pull request to a draft, or mark it ready for review, and
    /// read back whether it is a draft after. Only GraphQL does this, and
    /// it names the pull request by its node id, so the id is asked for
    /// first; one already as asked is left alone. The kept read of the
    /// pull request is forgotten, as a close does.
    pub async fn set_pull_request_draft(
        &self,
        repo_full_name: &str,
        number: u64,
        draft: bool,
    ) -> Result<bool, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;

        #[derive(serde::Deserialize)]
        struct Data {
            repository: Option<Repository>,
        }
        #[derive(serde::Deserialize)]
        struct Repository {
            #[serde(rename = "pullRequest")]
            pull_request: Option<PullNode>,
        }
        #[derive(serde::Deserialize)]
        struct PullNode {
            id: String,
            #[serde(rename = "isDraft")]
            is_draft: bool,
        }
        #[derive(serde::Deserialize)]
        struct Changed {
            #[serde(
                default,
                rename = "convertPullRequestToDraft",
                alias = "markPullRequestReadyForReview"
            )]
            payload: Option<ChangedPayload>,
        }
        #[derive(serde::Deserialize)]
        struct ChangedPayload {
            #[serde(rename = "pullRequest")]
            pull_request: Option<ChangedPull>,
        }
        #[derive(serde::Deserialize)]
        struct ChangedPull {
            #[serde(rename = "isDraft")]
            is_draft: bool,
        }

        let payload = serde_json::json!({
            "query": "query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) { id isDraft }
  }
}",
            "variables": { "owner": owner, "name": name, "number": number },
        });
        let data: Data = self.graphql(&payload, "pull request id").await?;
        let pull = data
            .repository
            .and_then(|repository| repository.pull_request)
            .ok_or_else(|| {
                Error::Decode(format!("GitHub has no pull request {repo_full_name}#{number}"))
            })?;
        if pull.is_draft == draft {
            return Ok(draft);
        }
        let field = if draft {
            "convertPullRequestToDraft"
        } else {
            "markPullRequestReadyForReview"
        };
        let payload = serde_json::json!({
            "query": format!(
                "mutation($pull: ID!) {{ {field}(input: {{pullRequestId: $pull}}) {{ pullRequest {{ isDraft }} }} }}"
            ),
            "variables": { "pull": pull.id },
        });
        let changed: Changed = self.graphql(&payload, "set draft").await?;
        self.forget_pull_request(repo_full_name, number);
        changed
            .payload
            .and_then(|payload| payload.pull_request)
            .map(|pull| pull.is_draft)
            .ok_or_else(|| Error::Decode("GitHub answered without the pull request".into()))
    }

    /// Forget the kept read of a pull request that just changed, under any
    /// casing of its repository's name, and keep no answer to a read of it
    /// asked before.
    fn forget_pull_request(&self, repo_full_name: &str, number: u64) {
        self.responses
            .lock()
            .unwrap()
            .remove_any_case(&format!("/repos/{repo_full_name}/pulls/{number}"));
    }

    /// Submit a final review of exactly the pull-request head the reader saw,
    /// with `comments` opening threads on its lines.
    ///
    /// GitHub accepts reviews of an older commit, so `commit_id` is carried
    /// explicitly rather than allowing the endpoint to default to whatever
    /// head is newest when the request arrives. The app performs its own
    /// fresh-head check immediately before calling this method; the explicit
    /// commit remains the final race-safe identity recorded by GitHub. A
    /// comment review with comments on lines and no summary is a review
    /// GitHub takes; with neither it is refused here, as a request for
    /// changes without a summary is.
    pub async fn submit_pull_request_review(
        &self,
        repo_full_name: &str,
        number: u64,
        commit_id: &str,
        body: &str,
        event: PullReviewEvent,
        comments: &[ReviewCommentDraft],
    ) -> Result<Review, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        if commit_id.is_empty() || commit_id.trim() != commit_id {
            return Err(Error::Decode(
                "pull request review commit SHA is empty or invalid".into(),
            ));
        }
        let summary_optional = event == PullReviewEvent::Comment && !comments.is_empty();
        if event.requires_body() && body.trim().is_empty() && !summary_optional {
            return Err(Error::Decode(format!(
                "a {event:?} pull request review requires a body"
            )));
        }

        #[derive(serde::Serialize)]
        struct Request<'a> {
            commit_id: &'a str,
            body: &'a str,
            event: PullReviewEvent,
            #[serde(skip_serializing_if = "<[ReviewCommentDraft]>::is_empty")]
            comments: &'a [ReviewCommentDraft],
        }

        let path = format!("/repos/{owner}/{name}/pulls/{number}/reviews");
        let request = Request {
            commit_id,
            body,
            event,
            comments,
        };
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._post(&path, Some(&request))
            .await
            .map_err(map_octocrab_error)?;
        let body = self
            .finish_response(response, &[StatusCode::OK], &format!("POST {path}"))
            .await?;
        let review: ReviewPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(Review::from(review))
    }

    /// Open a review thread on a line outside any review, against
    /// `commit_id`: what a comment written against a commit the pull
    /// request has moved past posts as. GitHub places it on that commit's
    /// diff.
    pub async fn create_review_comment(
        &self,
        repo_full_name: &str,
        number: u64,
        commit_id: &str,
        comment: &ReviewCommentDraft,
    ) -> Result<ReviewComment, Error> {
        if commit_id.is_empty() || commit_id.trim() != commit_id {
            return Err(Error::Decode(
                "pull request comment commit SHA is empty or invalid".into(),
            ));
        }

        #[derive(serde::Serialize)]
        struct Request<'a> {
            commit_id: &'a str,
            #[serde(flatten)]
            comment: &'a ReviewCommentDraft,
        }

        self.post_review_comment(
            repo_full_name,
            number,
            &Request {
                commit_id,
                comment,
            },
        )
        .await
    }

    /// Open a review thread on the file at `path` rather than on its
    /// lines, against `commit_id`: what GitHub takes for lines its pull
    /// request diff does not show. The comment says which lines itself
    /// (`comments::file_comment_body`).
    pub async fn create_file_comment(
        &self,
        repo_full_name: &str,
        number: u64,
        commit_id: &str,
        path: &str,
        body: &str,
    ) -> Result<ReviewComment, Error> {
        if commit_id.is_empty() || commit_id.trim() != commit_id {
            return Err(Error::Decode(
                "pull request comment commit SHA is empty or invalid".into(),
            ));
        }

        #[derive(serde::Serialize)]
        struct Request<'a> {
            commit_id: &'a str,
            path: &'a str,
            body: &'a str,
            subject_type: &'static str,
        }

        self.post_review_comment(
            repo_full_name,
            number,
            &Request {
                commit_id,
                path,
                body,
                subject_type: "file",
            },
        )
        .await
    }

    /// Reply under a review thread. GitHub takes the thread's root comment
    /// as `in_reply_to`, and the reply is not part of any review.
    pub async fn reply_to_review_comment(
        &self,
        repo_full_name: &str,
        number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<ReviewComment, Error> {
        if body.trim().is_empty() {
            return Err(Error::Decode("a reply requires a body".into()));
        }

        #[derive(serde::Serialize)]
        struct Request<'a> {
            body: &'a str,
            in_reply_to: u64,
        }

        self.post_review_comment(
            repo_full_name,
            number,
            &Request {
                body,
                in_reply_to: comment_id,
            },
        )
        .await
    }

    /// `POST /pulls/{number}/comments`, which answers `201 Created` with
    /// the comment it made.
    async fn post_review_comment<R: serde::Serialize>(
        &self,
        repo_full_name: &str,
        number: u64,
        request: &R,
    ) -> Result<ReviewComment, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        let path = format!("/repos/{owner}/{name}/pulls/{number}/comments");
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._post(&path, Some(request))
            .await
            .map_err(map_octocrab_error)?;
        let body = self
            .finish_response(response, &[StatusCode::CREATED], &format!("POST {path}"))
            .await?;
        let comment: ReviewCommentPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        // Its author may edit and delete it, as the reader now is.
        Ok(ReviewComment {
            viewer_can_delete: true,
            viewer_can_update: true,
            ..ReviewComment::from(comment)
        })
    }

    /// Say something in the conversation on a pull request or issue: an
    /// issue comment, `POST /issues/{number}/comments`, which answers
    /// `201 Created` with the comment it made. GitHub has no threads
    /// there, so a reply is a new comment like any other.
    pub async fn create_issue_comment(
        &self,
        repo_full_name: &str,
        number: u64,
        body: &str,
    ) -> Result<IssueComment, Error> {
        if body.trim().is_empty() {
            return Err(Error::Decode("a comment requires a body".into()));
        }
        let (owner, name) = repo_parts(repo_full_name)?;

        #[derive(serde::Serialize)]
        struct Request<'a> {
            body: &'a str,
        }

        let path = format!("/repos/{owner}/{name}/issues/{number}/comments");
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._post(&path, Some(&Request { body }))
            .await
            .map_err(map_octocrab_error)?;
        let body = self
            .finish_response(response, &[StatusCode::CREATED], &format!("POST {path}"))
            .await?;
        let comment: IssueCommentPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        // Its author may edit and delete it, as the reader now is.
        Ok(IssueComment {
            viewer_can_delete: true,
            viewer_can_update: true,
            ..IssueComment::from(comment)
        })
    }

    /// Open an issue on `repo_full_name`, `POST /issues`, which answers
    /// `201 Created` with the issue it made.
    pub async fn create_issue(
        &self,
        repo_full_name: &str,
        title: &str,
        body: &str,
    ) -> Result<Issue, Error> {
        self.create_issue_labeled(repo_full_name, title, body, &[])
            .await
    }

    /// [`Self::create_issue`] asking for `labels` too. GitHub drops them,
    /// and still makes the issue, for someone without push access.
    pub async fn create_issue_labeled(
        &self,
        repo_full_name: &str,
        title: &str,
        body: &str,
        labels: &[&str],
    ) -> Result<Issue, Error> {
        if title.trim().is_empty() {
            return Err(Error::Decode("an issue requires a title".into()));
        }
        let (owner, name) = repo_parts(repo_full_name)?;

        #[derive(serde::Serialize)]
        struct Request<'a> {
            title: &'a str,
            body: &'a str,
            #[serde(skip_serializing_if = "<[_]>::is_empty")]
            labels: &'a [&'a str],
        }

        let path = format!("/repos/{owner}/{name}/issues");
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._post(&path, Some(&Request { title, body, labels }))
            .await
            .map_err(map_octocrab_error)?;
        let body = self
            .finish_response(response, &[StatusCode::CREATED], &format!("POST {path}"))
            .await?;
        let payload: IssuePayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(payload.into_issue(repo_full_name))
    }

    /// The repository's newest hundred workflow runs, on any branch.
    ///
    /// No filter, so no search: GitHub answers a filtered listing from a
    /// search index that sometimes returns a stale or partial set, and
    /// this one from the runs themselves. A notification's run finished
    /// seconds before it was sent, so it is here unless the repository
    /// ran a hundred more since.
    pub async fn list_recent_workflow_runs(
        &self,
        repo_full_name: &str,
    ) -> Result<Vec<WorkflowRunPayload>, Error> {
        let body = self
            .get_text(&format!("/repos/{repo_full_name}/actions/runs?per_page=100"))
            .await?;
        let payload: WorkflowRunsPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(payload.workflow_runs)
    }

    /// The workflow runs on one branch created within `created`, a range
    /// of days as GitHub's search syntax writes one, newest first. What a
    /// workflow run notification's run is found among. One page: the run
    /// started shortly before the notification was sent, and a branch
    /// with a hundred newer runs in the same days is not one the reader
    /// is waiting on.
    pub async fn list_workflow_runs_for_branch(
        &self,
        repo_full_name: &str,
        branch: &str,
        created: &str,
    ) -> Result<Vec<WorkflowRunPayload>, Error> {
        let branch =
            percent_encoding::utf8_percent_encode(branch, percent_encoding::NON_ALPHANUMERIC);
        let body = self
            .get_text(&format!(
                "/repos/{repo_full_name}/actions/runs?branch={branch}&created={created}&per_page=100"
            ))
            .await?;
        let payload: WorkflowRunsPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(payload.workflow_runs)
    }

    /// Everything said on a pull request: description, conversation,
    /// reviews, and line comments. Three lists, each read to the end,
    /// what GitHub's timeline says was done to it, the state of each
    /// review thread, and the reactions on all of it. The timeline, the
    /// thread states and the reactions are the parts that may be missing:
    /// a discussion whose lists came is returned without them rather than
    /// not at all, and a missing timeline leaves the opening and ending
    /// the pull request's own payload carries.
    pub async fn fetch_discussion(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Discussion, Error> {
        let base = format!("/repos/{repo_full_name}");
        let pull_path = format!("{base}/pulls/{number}");
        let comments_path = format!("{base}/issues/{number}/comments");
        let reviews_path = format!("{base}/pulls/{number}/reviews");
        let review_comments_path = format!("{base}/pulls/{number}/comments");
        let (body, comments, reviews, review_comments) = tokio::try_join!(
            // An open or a refresh reads the pull request a moment before
            // it asks for the discussion; that read stands.
            self.get_text_within(&pull_path, JUST_READ),
            self.list_all::<IssueCommentPayload>(&comments_path),
            self.list_all::<ReviewPayload>(&reviews_path),
            self.list_all::<ReviewCommentPayload>(&review_comments_path),
        )?;
        let pull: PullSummary =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;

        let mut discussion = Discussion {
            viewer_can_update: false,
            events: pull.events(),
            node_id: pull.node_id,
            opened_at: pull.created_at,
            body: pull.body.unwrap_or_default(),
            reactions: Reactions::default(),
            comments: comments.into_iter().map(IssueComment::from).collect(),
            reviews: reviews.into_iter().map(Review::from).collect(),
            review_comments: review_comments
                .into_iter()
                .map(ReviewComment::from)
                .collect(),
            review_threads: Vec::new(),
        };
        let thread_states = async {
            if discussion.review_comments.is_empty() {
                return None;
            }
            Some(self.fetch_review_thread_states(repo_full_name, number).await)
        };
        let (timeline, thread_states) = tokio::join!(
            self.fetch_timeline(repo_full_name, number, &discussion.reviews),
            thread_states,
        );
        match timeline {
            // The timeline has no line for the opening, so that one stays.
            // It does carry the ending, so the payload's merge or close is
            // dropped where the timeline said the same thing, and kept
            // where it did not.
            Ok(events) => {
                discussion.events.retain(|own| {
                    !events.iter().any(|e| e.kind == own.kind && same_moment(e.at, own.at))
                });
                discussion.events.extend(events);
                discussion.events = tidy(std::mem::take(&mut discussion.events));
                // The timeline names a commit's author by git's name and
                // email; the commit list says whose account that is, and
                // is asked only when an email did not say.
                if wants_login(&discussion.events) {
                    match self
                        .list_all::<PullCommitPayload>(&format!("{base}/pulls/{number}/commits"))
                        .await
                    {
                        Ok(commits) => fill_logins(&mut discussion.events, &commits),
                        Err(e) => {
                            tracing::warn!(repo = repo_full_name, number, error = %e, "could not read the commits' authors");
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(repo = repo_full_name, number, error = %e, "could not read the timeline");
            }
        }
        match thread_states {
            Some(Ok(states)) => discussion.review_threads = states,
            Some(Err(e)) => {
                tracing::warn!(repo = repo_full_name, number, error = %e, "could not read the review threads");
            }
            None => {}
        }
        if let Err(e) = self.fill_reactions(&mut discussion).await {
            tracing::warn!(repo = repo_full_name, number, error = %e, "could not read the reactions");
        }
        Ok(discussion)
    }

    /// What was done to a pull request or an issue, in time order: the
    /// commits, the labels, the assignments, the review requests, the
    /// references and the changes of state, as GitHub's timeline lists
    /// them. One endpoint answers for both, since the two are numbered
    /// together.
    ///
    /// Everything Perch draws as a card of its own is left out, as is a
    /// kind this build has no line for. `reviews` is what a dismissal's
    /// review id is looked up in, since the event itself names no author;
    /// an issue has none and passes an empty slice.
    ///
    /// The list is heterogeneous, so each element is read on its own: one
    /// shape that does not fit costs its own line and not the page.
    ///
    /// GitHub lists the oldest first. A timeline longer than
    /// `TIMELINE_MAX_PAGES` pages is read for its first page and its
    /// newest, so how the item started and where it stands now are both
    /// there, and the pages between are what goes unread.
    pub async fn fetch_timeline(
        &self,
        repo_full_name: &str,
        number: u64,
        reviews: &[Review],
    ) -> Result<Vec<PullEvent>, Error> {
        let raw: Vec<serde_json::Value> = self
            .list_first_and_newest_pages(
                &format!("/repos/{repo_full_name}/issues/{number}/timeline"),
                TIMELINE_MAX_PAGES,
            )
            .await?;
        let reviewer = |id: u64| {
            reviews
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.user.clone())
                .filter(|user| !user.is_empty())
        };
        let mut events = Vec::new();
        // Each deleted comment's line, by where it is in `events`, and the
        // deletion's node id, which says whose comment it was.
        let mut deletions = Vec::new();
        for value in raw {
            let payload = match serde_json::from_value::<TimelineEventPayload>(value) {
                Ok(payload) => payload,
                Err(e) => {
                    tracing::debug!(repo = repo_full_name, number, error = %e, "skipping a timeline event that did not read");
                    continue;
                }
            };
            let node_id = payload.node_id.clone();
            let Some(event) = payload.into_event(&reviewer) else {
                continue;
            };
            if matches!(event.kind, PullEventKind::CommentDeleted { .. }) && !node_id.is_empty() {
                deletions.push((events.len(), node_id));
            }
            events.push(event);
        }
        if let Err(e) = self.fill_deleted_authors(&mut events, &deletions).await {
            tracing::info!(repo = repo_full_name, number, error = %e, "could not read whose comments were deleted");
        }
        Ok(tidy(events))
    }

    /// Name the author of each deleted comment, which the REST timeline
    /// leaves out, from GraphQL. A line GitHub says nothing more about
    /// keeps no name.
    async fn fill_deleted_authors(
        &self,
        events: &mut [PullEvent],
        deletions: &[(usize, String)],
    ) -> Result<(), Error> {
        for chunk in deletions.chunks(NODES_PER_QUERY) {
            let ids: Vec<&str> = chunk.iter().map(|(_, id)| id.as_str()).collect();
            let payload = serde_json::json!({
                "query": DELETED_AUTHORS_QUERY,
                "variables": { "ids": ids },
            });
            let data: DeletedAuthorsData = self.graphql(&payload, "deleted comment authors").await?;
            for ((at, _), login) in chunk.iter().zip(data.authors()) {
                if let Some(PullEventKind::CommentDeleted { author }) =
                    events.get_mut(*at).map(|e| &mut e.kind)
                {
                    *author = login;
                }
            }
        }
        Ok(())
    }

    /// Whether each review thread on a pull request is resolved, and
    /// whether GitHub considers it outdated, by the thread's first comment.
    /// The REST lists say neither. A hundred threads to a page, ten pages
    /// at most.
    pub async fn fetch_review_thread_states(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Vec<ReviewThreadState>, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        let number = i32::try_from(number)
            .map_err(|_| Error::Decode(format!("pull request number is too large: {number}")))?;
        let mut cursor = None::<String>;
        let mut states = Vec::new();
        for _ in 0..REVIEW_THREADS_MAX_PAGES {
            let payload = serde_json::json!({
                "query": REVIEW_THREADS_QUERY,
                "variables": {
                    "owner": owner,
                    "name": name,
                    "number": number,
                    "cursor": cursor.as_deref(),
                }
            });
            let (data, problems) = self
                .graphql_partial::<ReviewThreadsData>(&payload, "review threads")
                .await?;
            let Some(data) = data else {
                return Err(if problems.is_empty() {
                    Error::Decode("GitHub's GraphQL response had no data".into())
                } else {
                    Error::Graphql(problems.join("; "))
                });
            };
            let Some((page, next)) = data.into_page() else {
                return Err(if problems.is_empty() {
                    Error::Decode(format!(
                        "GitHub returned no pull request for {repo_full_name}#{number}"
                    ))
                } else {
                    Error::Graphql(problems.join("; "))
                });
            };
            for problem in problems {
                tracing::debug!(problem, "part of the review threads could not be read");
            }
            states.extend(page);
            let Some(next) = next else { return Ok(states) };
            if cursor.as_deref() == Some(next.as_str()) {
                return Err(Error::Decode(
                    "GitHub repeated a review thread page cursor".into(),
                ));
            }
            cursor = Some(next);
        }
        tracing::warn!(
            repo = repo_full_name,
            number,
            "more review threads than {REVIEW_THREADS_MAX_PAGES} pages hold; the rest go unread"
        );
        Ok(states)
    }

    /// Resolve a review thread, or unresolve it, by the thread's GraphQL
    /// id, and read back whether it is resolved after. Asking for what the
    /// thread already is is not an error: GitHub answers with it as it is.
    pub async fn set_review_thread_resolved(
        &self,
        node_id: &str,
        resolve: bool,
    ) -> Result<bool, Error> {
        if node_id.is_empty() {
            return Err(Error::Decode("the thread has no node id".into()));
        }
        let payload = serde_json::json!({
            "query": resolve_thread_mutation(resolve),
            "variables": { "thread": node_id },
        });
        let data: ResolveThreadData = self.graphql(&payload, "resolve thread").await?;
        data.into_resolved()
            .ok_or_else(|| Error::Decode("GitHub answered without the thread".into()))
    }

    /// The text of `path` at commit `sha` of `repo_full_name`, or why it
    /// has none: not there, binary, or too big for GitHub to send whole.
    pub async fn file_text(
        &self,
        repo_full_name: &str,
        sha: &str,
        path: &str,
    ) -> Result<Result<String, Unreadable>, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        let payload = serde_json::json!({
            "query": FILE_TEXT_QUERY,
            "variables": { "owner": owner, "name": name, "expression": format!("{sha}:{path}") },
        });
        let data: FileTextData = self.graphql(&payload, "file text").await?;
        Ok(data.into_text())
    }

    /// Commit `contents` as the whole of `path` on `branch` of
    /// `repo_full_name`, the head repository of a pull request, as
    /// github.com commits an applied suggestion. GitHub refuses it when
    /// the branch is no longer at `expected_head`. The new commit's sha.
    #[allow(clippy::too_many_arguments)]
    pub async fn commit_file(
        &self,
        repo_full_name: &str,
        branch: &str,
        expected_head: &str,
        path: &str,
        contents: &str,
        headline: &str,
        body: &str,
    ) -> Result<String, Error> {
        if headline.trim().is_empty() {
            return Err(Error::Decode("a commit needs a message".into()));
        }
        let payload = serde_json::json!({
            "query": COMMIT_ON_BRANCH_MUTATION,
            "variables": commit_variables(
                repo_full_name, branch, expected_head, path, contents, headline, body,
            ),
        });
        let data: CommitOnBranchData = self.graphql(&payload, "commit on branch").await?;
        data.into_oid()
            .ok_or_else(|| Error::Decode("GitHub answered the commit without it".into()))
    }

    /// The reactions on everything in a discussion, as the reader sees
    /// them: how many of each, and which are the reader's own. The lists
    /// count reactions without saying whose they are, and count none on a
    /// review or the body, so GraphQL is asked about every node at once.
    ///
    /// The same query asks which comments the reader may edit and delete.
    pub async fn fill_reactions(&self, discussion: &mut Discussion) -> Result<(), Error> {
        let ids = discussion.reactable_ids();
        for (id, node) in self.read_nodes(&ids).await? {
            discussion.set_reactions(&id, &node.reactions);
            discussion.set_deletable(&id, node.viewer_can_delete);
            discussion.set_updatable(&id, node.viewer_can_update);
        }
        Ok(())
    }

    /// The reactions on each of the nodes named, by id, a hundred to a
    /// query. A node GitHub cannot resolve, or that cannot be reacted to,
    /// is left out of the answer.
    pub async fn fetch_reactions(
        &self,
        node_ids: &[String],
    ) -> Result<std::collections::HashMap<String, Reactions>, Error> {
        Ok(self
            .read_nodes(node_ids)
            .await?
            .into_iter()
            .map(|(id, node)| (id, node.reactions))
            .collect())
    }

    /// The reactions on each of the nodes named and whether the reader
    /// may delete it, by id, a hundred to a query.
    async fn read_nodes(
        &self,
        node_ids: &[String],
    ) -> Result<std::collections::HashMap<String, NodeReading>, Error> {
        let mut all = std::collections::HashMap::new();
        for ids in node_ids.chunks(NODES_PER_QUERY) {
            let payload = serde_json::json!({
                "query": reactions_query(),
                "variables": { "ids": ids },
            });
            let data: ReactionsData = self.graphql(&payload, "reactions").await?;
            all.extend(data.by_id(ids));
        }
        Ok(all)
    }

    /// Add the reader's reaction to a subject, or take it back, and read
    /// the subject's reactions as they stand after. Adding one the reader
    /// already left, or taking back one they did not, is not an error:
    /// GitHub answers with the reactions as they are.
    pub async fn set_reaction(
        &self,
        node_id: &str,
        kind: ReactionKind,
        add: bool,
    ) -> Result<Reactions, Error> {
        if node_id.is_empty() {
            return Err(Error::Decode("the subject has no node id".into()));
        }
        let payload = serde_json::json!({
            "query": reaction_mutation(add),
            "variables": { "subject": node_id, "content": kind.graphql() },
        });
        let data: ReactionMutationData = self.graphql(&payload, "react").await?;
        data.into_reactions()
            .ok_or_else(|| Error::Decode("GitHub answered the reaction without its subject".into()))
    }

    /// Delete a comment on GitHub, for good, by its GraphQL id: one in
    /// the conversation, or one on a line of a pull request. GitHub
    /// refuses a reader who is neither its author nor able to write to
    /// the repository.
    pub async fn delete_comment(&self, node_id: &str, place: CommentPlace) -> Result<(), Error> {
        if node_id.is_empty() {
            return Err(Error::Decode("the comment has no node id".into()));
        }
        let field = match place {
            CommentPlace::Conversation => "deleteIssueComment",
            CommentPlace::Line => "deletePullRequestReviewComment",
            CommentPlace::PullRequestBody | CommentPlace::IssueBody | CommentPlace::Review => {
                return Err(Error::Decode("a description or a review is not deleted".into()));
            }
        };
        let payload = serde_json::json!({
            "query": format!(
                "mutation($id: ID!) {{ {field}(input: {{id: $id}}) {{ clientMutationId }} }}"
            ),
            "variables": { "id": node_id },
        });
        let _: serde_json::Value = self.graphql(&payload, "delete comment").await?;
        Ok(())
    }

    /// Edit a comment on GitHub, by its GraphQL id: one in the
    /// conversation, one on a line of a pull request, a pull request's or
    /// an issue's description, or a review's summary. The answer is the
    /// body GitHub keeps, and for a description when its pull request or
    /// issue last changed. GitHub refuses a reader who is neither its
    /// author nor able to write to the repository.
    ///
    /// A description's edit forgets the kept read of its pull request or
    /// issue, and keeps no answer to a read of it asked before, so the
    /// discussion read after it asks GitHub rather than taking a read from
    /// moments before the edit as current.
    pub async fn update_comment(
        &self,
        node_id: &str,
        place: CommentPlace,
        body: &str,
    ) -> Result<EditedComment, Error> {
        if node_id.is_empty() {
            return Err(Error::Decode("the comment has no node id".into()));
        }
        // The last is where a description's subject is read from, by
        // repository and number.
        let (field, id, comment, read_at) = match place {
            CommentPlace::Conversation => ("updateIssueComment", "id", "issueComment", None),
            CommentPlace::Line => (
                "updatePullRequestReviewComment",
                "pullRequestReviewCommentId",
                "pullRequestReviewComment",
                None,
            ),
            CommentPlace::PullRequestBody => {
                ("updatePullRequest", "pullRequestId", "pullRequest", Some("pulls"))
            }
            CommentPlace::IssueBody => ("updateIssue", "id", "issue", Some("issues")),
            CommentPlace::Review => (
                "updatePullRequestReview",
                "pullRequestReviewId",
                "pullRequestReview",
                None,
            ),
        };
        let fields = if read_at.is_some() {
            "body number repository { nameWithOwner } updatedAt"
        } else {
            "body"
        };
        let payload = serde_json::json!({
            "query": format!(
                "mutation($id: ID!, $body: String!) {{ {field}(input: {{{id}: $id, body: $body}}) {{ {comment} {{ {fields} }} }} }}"
            ),
            "variables": { "id": node_id, "body": body },
        });
        let data: serde_json::Value = self.graphql(&payload, "edit comment").await?;
        let answer = data.get(field).and_then(|answer| answer.get(comment));
        let kept = answer
            .and_then(|comment| comment.get("body"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::Decode("GitHub answered the edit without the comment".into()))?;
        let updated_at = answer
            .and_then(|comment| comment.get("updatedAt"))
            .and_then(serde_json::Value::as_str)
            .and_then(|at| at.parse().ok());
        if let (Some(read_at), Some(answer)) = (read_at, answer) {
            let number = answer.get("number").and_then(serde_json::Value::as_u64);
            let repo = answer
                .pointer("/repository/nameWithOwner")
                .and_then(serde_json::Value::as_str);
            if let (Some(number), Some(repo)) = (number, repo) {
                self.responses
                    .lock()
                    .unwrap()
                    .remove_any_case(&format!("/repos/{repo}/{read_at}/{number}"));
            }
        }
        Ok(EditedComment {
            body: kept,
            updated_at,
        })
    }

    /// Whether the reader gave each of these issues a 👍, how many did,
    /// and whether they hear about it, in one query. An issue GitHub
    /// can't find is left out.
    pub async fn issue_viewer_states(
        &self,
        repo_full_name: &str,
        numbers: &[u64],
    ) -> Result<Vec<crate::subscription::IssueViewerState>, Error> {
        if numbers.is_empty() {
            return Ok(Vec::new());
        }
        let (owner, name) = repo_parts(repo_full_name)?;
        let payload = serde_json::json!({
            "query": crate::subscription::issues_query(numbers),
            "variables": { "owner": owner, "name": name },
        });
        let (data, _problems) = self
            .graphql_partial::<crate::subscription::IssuesData>(&payload, "issue viewer states")
            .await?;
        Ok(data.map(|d| d.into_states(numbers)).unwrap_or_default())
    }

    /// Subscribe the reader to a subject, or unsubscribe or ignore it:
    /// `state` is `SUBSCRIBED`, `UNSUBSCRIBED` or `IGNORED`. Answers the
    /// state GitHub ended with.
    pub async fn set_subscription(&self, node_id: &str, state: &str) -> Result<String, Error> {
        if node_id.is_empty() {
            return Err(Error::Decode("the subject has no node id".into()));
        }
        let payload = serde_json::json!({
            "query": crate::subscription::SUBSCRIPTION_MUTATION,
            "variables": { "id": node_id, "state": state },
        });
        let data: crate::subscription::SubscriptionData =
            self.graphql(&payload, "subscribe").await?;
        data.state()
            .ok_or_else(|| Error::Decode("GitHub answered the subscription without its state".into()))
    }

    /// When the rate limit GitHub last named lifts, as seconds since the
    /// epoch, while it's still in force.
    pub fn rate_limited_until(&self) -> Option<i64> {
        let until = self.rate_limit_until.load(Ordering::SeqCst);
        (until > jiff::Timestamp::now().as_second()).then_some(until)
    }

    /// The repository's users an `@` can name whose login or name
    /// matches `query`, at most `first` of them, in GitHub's order: who
    /// its own comment box suggests.
    pub async fn fetch_mentionable_users(
        &self,
        repo_full_name: &str,
        query: &str,
        first: usize,
    ) -> Result<Vec<crate::MentionableUser>, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        let payload = serde_json::json!({
            "query": crate::mentions::mentionable_users_query(),
            "variables": {
                "owner": owner,
                "name": name,
                "query": (!query.is_empty()).then_some(query),
                "first": first.clamp(1, 100),
            },
        });
        let data: crate::mentions::MentionableUsersData = self
            .graphql(&payload, "mentionable users")
            .await?;
        Ok(data.into_users())
    }

    /// The repository's issues and pull requests a `#` can name for
    /// `query`, at most `first` found by the search, and the one with the
    /// number first when the query is one: who its own comment box
    /// suggests.
    pub async fn fetch_issue_references(
        &self,
        repo_full_name: &str,
        query: &str,
        first: usize,
    ) -> Result<Vec<crate::IssueReference>, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        let number = query.parse::<i32>().ok().filter(|n| *n > 0);
        let payload = serde_json::json!({
            "query": crate::references::issue_references_query(),
            "variables": {
                "owner": owner,
                "name": name,
                "search": crate::references::search_terms(repo_full_name, query),
                "first": first.clamp(1, 100),
                "number": number.unwrap_or(1),
                "byNumber": number.is_some(),
            },
        });
        // A number nobody has is an error beside the search's answer.
        let (data, problems) = self
            .graphql_partial::<crate::references::IssueReferencesData>(&payload, "issue references")
            .await?;
        match data {
            Some(data) => Ok(data.into_references()),
            None if problems.is_empty() => {
                Err(Error::Decode("GitHub's GraphQL response had no data".into()))
            }
            None => Err(Error::Graphql(problems.join("; "))),
        }
    }

    /// Fetch an issue by number.
    ///
    /// The issues endpoint answers for pull requests too, because GitHub
    /// numbers the two together. One is refused here, so nothing that calls
    /// this can store a pull request as an issue.
    pub async fn fetch_issue(&self, repo_full_name: &str, number: u64) -> Result<Issue, Error> {
        let body = self
            .get_text(&format!("/repos/{repo_full_name}/issues/{number}"))
            .await?;
        let payload: IssuePayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        if payload.is_pull_request() {
            return Err(Error::Decode(format!(
                "{repo_full_name}#{number} is a pull request"
            )));
        }
        Ok(payload.into_issue(repo_full_name))
    }

    /// Close an issue: `PATCH /issues/{number}` with `state: closed` and
    /// the reason, which answers with the issue as it now stands. A
    /// duplicate names the original by its database id, read first by
    /// its number.
    pub async fn close_issue(
        &self,
        repo_full_name: &str,
        number: u64,
        reason: IssueCloseReason,
    ) -> Result<Issue, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        let duplicate_issue_id = match reason {
            IssueCloseReason::Duplicate(original) if original == number => {
                return Err(Error::Decode("an issue cannot duplicate itself".into()));
            }
            IssueCloseReason::Duplicate(original) => {
                let body = self
                    .get_text(&format!("/repos/{owner}/{name}/issues/{original}"))
                    .await?;
                let payload: IssuePayload =
                    serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
                if payload.is_pull_request() {
                    return Err(Error::Decode(format!("#{original} is a pull request")));
                }
                Some(payload.id)
            }
            _ => None,
        };

        #[derive(serde::Serialize)]
        struct Request {
            state: &'static str,
            state_reason: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            duplicate_issue_id: Option<u64>,
        }

        let path = format!("/repos/{owner}/{name}/issues/{number}");
        let request = Request {
            state: "closed",
            state_reason: reason.as_str(),
            duplicate_issue_id,
        };
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._patch(&path, Some(&request))
            .await
            .map_err(map_octocrab_error)?;
        let body = self
            .finish_response(response, &[StatusCode::OK], &format!("PATCH {path}"))
            .await?;
        let payload: IssuePayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(payload.into_issue(repo_full_name))
    }

    /// Whether the reader may close an issue: its author may, and so may
    /// anyone with triage access to its repository. GitHub works it out,
    /// as `viewerCanClose`.
    pub async fn viewer_can_close_issue(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<bool, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;

        #[derive(serde::Deserialize)]
        struct Data {
            repository: Option<Repository>,
        }
        #[derive(serde::Deserialize)]
        struct Repository {
            issue: Option<IssueNode>,
        }
        #[derive(serde::Deserialize)]
        struct IssueNode {
            viewer_can_close: bool,
        }

        let payload = serde_json::json!({
            "query": "query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) { viewer_can_close: viewerCanClose }
  }
}",
            "variables": { "owner": owner, "name": name, "number": number },
        });
        let data: Data = self.graphql(&payload, "viewer can close").await?;
        data.repository
            .and_then(|repository| repository.issue)
            .map(|issue| issue.viewer_can_close)
            .ok_or_else(|| Error::Decode(format!("GitHub has no issue {repo_full_name}#{number}")))
    }

    /// The repository's assignable users, at most `first` of them, when
    /// the reader may assign them; none when they may not. GitHub lets
    /// triage access and more assign, whoever opened the issue.
    pub async fn assignable_users(
        &self,
        repo_full_name: &str,
        first: usize,
    ) -> Result<Option<Vec<crate::MentionableUser>>, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;
        let payload = serde_json::json!({
            "query": crate::assignees::assignable_query(),
            "variables": { "owner": owner, "name": name, "first": first.clamp(1, 100) },
        });
        let data: crate::assignees::AssignableData =
            self.graphql(&payload, "assignable users").await?;
        Ok(data.into_assignable())
    }

    /// Assign `logins` to an issue or pull request, or take them off it
    /// when `assign` is false: `POST` or `DELETE /issues/{number}/assignees`,
    /// which leave anyone else assigned as they were and answer with the
    /// issue. GitHub drops a login it will not assign without a word, so
    /// the answer is the one to believe.
    pub async fn set_assignees(
        &self,
        repo_full_name: &str,
        number: u64,
        logins: &[String],
        assign: bool,
    ) -> Result<crate::Assigned, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;

        #[derive(serde::Serialize)]
        struct Request<'a> {
            assignees: &'a [String],
        }

        let path = format!("/repos/{owner}/{name}/issues/{number}/assignees");
        let request = Request { assignees: logins };
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let (response, method, expected) = if assign {
            let response = self.octo._post(&path, Some(&request)).await;
            (response, "POST", StatusCode::CREATED)
        } else {
            let response = self.octo._delete(&path, Some(&request)).await;
            (response, "DELETE", StatusCode::OK)
        };
        let response = response.map_err(map_octocrab_error)?;
        let body = self
            .finish_response(response, &[expected, StatusCode::OK], &format!("{method} {path}"))
            .await?;
        let payload: IssuePayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(crate::Assigned {
            assignees: payload.assignees.into_iter().map(|u| u.login).collect(),
            updated_at: payload.updated_at,
        })
    }

    /// Whether a number in a repository is a pull request or an issue.
    /// GitHub numbers the two together, and the issues endpoint answers
    /// for both, with a `pull_request` key on a pull request.
    pub async fn subject_kind(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<crate::items::SubjectKind, Error> {
        repo_parts(repo_full_name)?;
        let body = self
            .get_text(&format!("/repos/{repo_full_name}/issues/{number}"))
            .await?;
        let payload: IssuePayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(if payload.is_pull_request() {
            crate::items::SubjectKind::PullRequest
        } else {
            crate::items::SubjectKind::Issue
        })
    }

    /// One page of GitHub's issue search, which finds pull requests too,
    /// most recently updated first unless the query says otherwise with
    /// a `sort:` qualifier, as github.com's search box takes one. `page`
    /// counts from 1.
    ///
    /// The search API has a window of its own. A search that finds it
    /// spent is refused until GitHub's reset, without asking, and no
    /// other request is held up by it.
    pub async fn search_issues(
        &self,
        query: &str,
        page: u32,
        per_page: u32,
    ) -> Result<crate::search::SearchPage, Error> {
        let (query, sort, order) = search_sort(query);
        let q = percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC);
        let path = format!("/search/issues?q={q}&sort={sort}&order={order}&per_page={per_page}&page={page}");
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let until = self.search_limit_until.load(Ordering::SeqCst);
        if until > Timestamp::now().as_second() {
            return Err(Error::RateLimited {
                reset_epoch: Some(until),
                retry_after_secs: None,
            });
        }
        let response = self
            .octo
            ._get_with_headers(&path, None)
            .await
            .map_err(map_octocrab_error)?;
        let status = response.status();
        let (remaining, reset, retry_after) = rate_headers(response.headers());
        self.note_spend(&format!("GET {path}"), status, response.headers(), None);
        let body = body_text(response).await;
        let spent = |reset_epoch, retry_after_secs| {
            self.search_limit_until.fetch_max(
                rate_limit_deadline(reset_epoch, retry_after_secs),
                Ordering::SeqCst,
            );
        };
        if !status.is_success() {
            let error = classify(status, remaining, reset, retry_after, body);
            if let Error::RateLimited {
                reset_epoch,
                retry_after_secs,
            } = &error
            {
                spent(*reset_epoch, *retry_after_secs);
            }
            return Err(error);
        }
        if remaining == Some(0) || retry_after.is_some() {
            spent(reset, retry_after);
        }
        let payload: crate::search::SearchPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(payload.into_page(page, per_page))
    }

    /// Who the token belongs to: the login and the avatar, for the
    /// reader's own byline.
    pub async fn viewer(&self) -> Result<crate::model::UserPayload, Error> {
        let body = self.get_text("/user").await?;
        serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))
    }

    /// A user or organization by login, or `Error::Status` 404 when there
    /// is none.
    pub async fn user(&self, login: &str) -> Result<crate::model::UserPayload, Error> {
        let body = self.get_text(&format!("/users/{login}")).await?;
        serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))
    }

    /// The conversation on an issue, read to the end.
    pub async fn fetch_issue_comments(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<Vec<IssueComment>, Error> {
        let comments: Vec<IssueCommentPayload> = self
            .list_all(&format!("/repos/{repo_full_name}/issues/{number}/comments"))
            .await?;
        Ok(comments.into_iter().map(IssueComment::from).collect())
    }

    /// Who opened each thread's pull request or issue, and the state it is
    /// in, asked for in one GraphQL query and answered by thread id.
    ///
    /// Threads of any other kind, and those without a number, are passed
    /// over, as is a subject GitHub cannot resolve: one that was deleted,
    /// or whose repository the token cannot see. GitHub reports those as
    /// errors beside the rest of the answer, which is still read. The
    /// caller picks the batch: fifty or so is one query GitHub answers at
    /// once.
    pub async fn look_at_subjects(
        &self,
        threads: &[Thread],
    ) -> Result<Vec<(String, SubjectSnapshot)>, Error> {
        let Some(look) = SubjectLook::new(threads) else {
            return Ok(Vec::new());
        };
        let payload = serde_json::json!({ "query": look.query() });
        let (data, problems) = self
            .graphql_partial::<SubjectLookData>(&payload, "subject looks")
            .await?;
        let Some(data) = data else {
            return Err(if problems.is_empty() {
                Error::Decode("GitHub's GraphQL response had no data".into())
            } else {
                Error::Graphql(problems.join("; "))
            });
        };
        for problem in problems {
            tracing::debug!(problem, "a notification's subject could not be looked at");
        }
        Ok(look.read(data))
    }

    /// Who acted last on each of these pull requests and issues, named by
    /// repository and number, in the query `look_at_subjects` sends: fifty
    /// or so a request. A subject GitHub cannot resolve is passed over.
    pub async fn look_at_participants(
        &self,
        subjects: &[(String, u64)],
    ) -> Result<Vec<((String, u64), Vec<crate::model::Participant>)>, Error> {
        let Some(look) =
            SubjectLook::of_subjects(subjects.iter().map(|(repo, number)| (repo.as_str(), *number)))
        else {
            return Ok(Vec::new());
        };
        let payload = serde_json::json!({ "query": look.query() });
        let (data, problems) = self
            .graphql_partial::<SubjectLookData>(&payload, "subject participants")
            .await?;
        let Some(data) = data else {
            return Err(if problems.is_empty() {
                Error::Decode("GitHub's GraphQL response had no data".into())
            } else {
                Error::Graphql(problems.join("; "))
            });
        };
        for problem in problems {
            tracing::debug!(problem, "a subject's participants could not be looked at");
        }
        Ok(look.participants(data))
    }

    /// The state of each of these pull requests and issues, named by
    /// repository and number: fifty or so a request, which GitHub counts
    /// as one point. A subject GitHub cannot resolve is passed over.
    pub async fn look_at_states(
        &self,
        subjects: &[(String, u64)],
    ) -> Result<Vec<((String, u64), SubjectState)>, Error> {
        let Some(look) =
            SubjectLook::of_subjects(subjects.iter().map(|(repo, number)| (repo.as_str(), *number)))
        else {
            return Ok(Vec::new());
        };
        let payload = serde_json::json!({ "query": look.state_query() });
        let (data, problems) = self
            .graphql_partial::<SubjectLookData>(&payload, "subject states")
            .await?;
        let Some(data) = data else {
            return Err(if problems.is_empty() {
                Error::Decode("GitHub's GraphQL response had no data".into())
            } else {
                Error::Graphql(problems.join("; "))
            });
        };
        for problem in problems {
            tracing::debug!(problem, "a subject's state could not be looked at");
        }
        Ok(look.states(data))
    }

    /// The pull request whose head is `owner:branch` on `repo_full_name`,
    /// open or closed, with a conditional request: an unchanged answer
    /// costs a 304 and returns `Unchanged`.
    pub async fn find_pull_for_branch(
        &self,
        repo_full_name: &str,
        head_owner: &str,
        branch: &str,
        etag: Option<&str>,
    ) -> Result<PullLookup, Error> {
        let head_raw = format!("{head_owner}:{branch}");
        let head =
            percent_encoding::utf8_percent_encode(&head_raw, percent_encoding::NON_ALPHANUMERIC);
        let path = format!("/repos/{repo_full_name}/pulls?head={head}&state=all&per_page=5");

        let fetched = self.get(&path, conditional(etag)).await?;
        if fetched.status == StatusCode::NOT_MODIFIED {
            return Ok(PullLookup::Unchanged);
        }
        let new_etag = header_str(&fetched.headers, ETAG.as_str());
        let payloads: Vec<PullPayload> =
            serde_json::from_str(&fetched.body).map_err(|e| Error::Decode(e.to_string()))?;
        // Open first, else the newest: a branch that had a pull request
        // closed and reopened as another should show the live one.
        let pick = payloads
            .iter()
            .position(|p| p.state.eq_ignore_ascii_case("open"))
            .or(if payloads.is_empty() { None } else { Some(0) });
        let pull = pick.map(|i| {
            let mut all = payloads;
            Box::new(all.swap_remove(i).into_pull_request(repo_full_name))
        });
        Ok(PullLookup::Answer {
            pull,
            etag: new_etag,
        })
    }

    /// The repository at the top of `repo_full_name`'s forks: the one it
    /// was forked from, all the way up, or itself when it is not a fork.
    /// A pull request only goes between two repositories with the same
    /// top.
    pub async fn fork_root(&self, repo_full_name: &str) -> Result<String, Error> {
        #[derive(serde::Deserialize)]
        struct Repository {
            full_name: String,
            source: Option<Source>,
        }
        #[derive(serde::Deserialize)]
        struct Source {
            full_name: String,
        }

        let body = self.get_text(&format!("/repos/{repo_full_name}")).await?;
        let repo: Repository =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(repo.source.map_or(repo.full_name, |s| s.full_name))
    }

    /// Whether a commit's check runs or commit statuses may have changed
    /// since `tags` were read: both listings asked conditionally, so a
    /// quiet commit costs two 304s and none of the rate limit. The
    /// listings are read only to say whether they agree with a rollup
    /// already read; the rollup is what is shown. Only their first page is
    /// asked for, so a change past a commit's hundredth check goes unseen
    /// until something on the first page moves.
    pub async fn probe_checks(
        &self,
        repo_full_name: &str,
        sha: &str,
        tags: &CheckTags,
    ) -> Result<CheckProbe, Error> {
        let runs = format!("/repos/{repo_full_name}/commits/{sha}/check-runs?per_page=100");
        let statuses = format!("/repos/{repo_full_name}/commits/{sha}/status?per_page=100");
        let (runs_tag, runs_body) = self.probe(&runs, tags.runs.as_deref()).await?;
        let (statuses_tag, statuses_body) =
            self.probe(&statuses, tags.statuses.as_deref()).await?;
        Ok(CheckProbe {
            tags: CheckTags {
                runs: runs_tag,
                statuses: statuses_tag,
            },
            seen: runs_body
                .zip(statuses_body)
                .and_then(|(runs, statuses)| rest_check_sighting(&runs, &statuses)),
        })
    }

    /// The tag `path` answers with now, and the body when it came: the old
    /// tag on a 304, else its `ETag`, else a hash of the body so a change
    /// can still be told.
    async fn probe(
        &self,
        path: &str,
        tag: Option<&str>,
    ) -> Result<(Option<String>, Option<String>), Error> {
        let fetched = self.get(path, conditional(tag)).await?;
        if fetched.status == StatusCode::NOT_MODIFIED {
            return Ok((tag.map(str::to_string), None));
        }
        let tag = header_str(&fetched.headers, ETAG.as_str()).unwrap_or_else(|| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            fetched.body.hash(&mut hasher);
            format!("body:{:x}", hasher.finish())
        });
        Ok((Some(tag), Some(fetched.body)))
    }

    /// Open a pull request on `repo_full_name` from `head`, which is
    /// `owner:branch`, into `base`. The branch must be pushed already.
    pub async fn create_pull_request(
        &self,
        repo_full_name: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<PullRequest, Error> {
        let (owner, name) = repo_parts(repo_full_name)?;

        #[derive(serde::Serialize)]
        struct Request<'a> {
            title: &'a str,
            body: &'a str,
            head: &'a str,
            base: &'a str,
            draft: bool,
        }

        let path = format!("/repos/{owner}/{name}/pulls");
        let request = Request {
            title,
            body,
            head,
            base,
            draft,
        };
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._post(&path, Some(&request))
            .await
            .map_err(map_octocrab_error)?;
        let body = self
            .finish_response(response, &[StatusCode::CREATED], &format!("POST {path}"))
            .await?;
        let payload: PullPayload =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        Ok(payload.into_pull_request(repo_full_name))
    }

    /// Every page of a list endpoint, concatenated.
    ///
    /// Stops at the first short page rather than following `Link` headers,
    /// which is what `list_threads` does too. A hundred a page and a
    /// hundred pages is more discussion than any pull request has.
    async fn list_all<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, Error> {
        let mut all = Vec::new();
        for page in 1..=LIST_MAX_PAGES {
            let body = self.get_text(&page_path(path, page)).await?;
            let items: Vec<T> =
                serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
            let short = items.len() < LIST_PER_PAGE;
            all.extend(items);
            if short {
                break;
            }
        }
        Ok(all)
    }

    /// A list GitHub keeps oldest first, cut to `max_pages` pages when it
    /// runs longer: the first page, and the newest after it. The first is
    /// read anyway, since its `Link` header says where the list ends, so
    /// keeping it costs nothing; the pages between are the ones skipped.
    ///
    /// A 304 on the first page can hand back a `Link` kept from when the
    /// list was shorter, so pages past the last one it names are read too
    /// while they come full, and the oldest of the newest are let go.
    async fn list_first_and_newest_pages<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        max_pages: usize,
    ) -> Result<Vec<T>, Error> {
        let (body, link) = self.get_linked(&page_path(path, 1)).await?;
        let mut all: Vec<T> =
            serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
        let newest = max_pages.saturating_sub(1);
        if all.len() < LIST_PER_PAGE || newest == 0 {
            return Ok(all);
        }
        let first = newest_pages_from(link.as_deref().and_then(last_page), newest);
        let mut tail = std::collections::VecDeque::new();
        for page in first..first.saturating_add(LIST_MAX_PAGES) {
            let body = self.get_text(&page_path(path, page)).await?;
            let items: Vec<T> =
                serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
            if items.is_empty() {
                break;
            }
            let short = items.len() < LIST_PER_PAGE;
            tail.push_back(items);
            if tail.len() > newest {
                tail.pop_front();
            }
            if short {
                break;
            }
        }
        all.extend(tail.into_iter().flatten());
        Ok(all)
    }

    /// One GET whose body is all the caller wants.
    ///
    /// Asked with the tag of the body last read at `path`, if one is kept:
    /// a 304 hands that body back, so the caller cannot tell the two apart
    /// except by the rate limit it did not spend.
    async fn get_text(&self, path: &str) -> Result<String, Error> {
        self.get_linked(path).await.map(|(body, _)| body)
    }

    /// `get_text`, and the `Link` header the body came with: the answer's
    /// own when it has one, else, on a 304, the one kept beside the body.
    /// A kept one can be out of date, since a page's tag says nothing of
    /// the pages after it.
    ///
    /// An answer to a read asked before a change forgot `path` goes to
    /// the caller alone: GitHub may have answered from before the change,
    /// and kept, the answer would count as current.
    async fn get_linked(&self, path: &str) -> Result<(String, Option<String>), Error> {
        let (known, forgets) = {
            let mut responses = self.responses.lock().unwrap();
            (responses.get(path), responses.forgets())
        };
        let fetched = self
            .get(path, conditional(known.as_ref().map(|k| k.etag.as_str())))
            .await?;
        let link = header_str(&fetched.headers, LINK.as_str());
        let mut responses = self.responses.lock().unwrap();
        let keep = !responses.forgotten_since(path, forgets);
        if fetched.status == StatusCode::NOT_MODIFIED {
            if keep {
                responses.confirm(path);
            }
            return match known {
                Some(known) => Ok((known.body.to_string(), link.or(known.link))),
                None => Err(Error::Status {
                    status: StatusCode::NOT_MODIFIED.as_u16(),
                    body: String::new(),
                }),
            };
        }
        if keep {
            match header_str(&fetched.headers, ETAG.as_str()) {
                Some(etag) => responses.put(
                    path,
                    Known {
                        etag,
                        body: fetched.body.as_str().into(),
                        link: link.clone(),
                    },
                ),
                None => responses.remove(path),
            }
        }
        Ok((fetched.body, link))
    }

    /// `get_text`, without asking when GitHub said the kept body was
    /// current no longer than `within` ago.
    async fn get_text_within(&self, path: &str, within: Duration) -> Result<String, Error> {
        let fresh = self.responses.lock().unwrap().fresh(path, within);
        match fresh {
            Some(known) => Ok(known.body.to_string()),
            None => self.get_text(path).await,
        }
    }

    /// One GET, classified the way every request is.
    ///
    /// A 2xx comes back with its body; a 304 comes back with an empty one,
    /// for the callers that asked conditionally; anything else is an error.
    /// Rate-limit response headers are still used to classify an error.
    async fn get(&self, path: &str, headers: Option<HeaderMap>) -> Result<Fetched, Error> {
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._get_with_headers(path, headers)
            .await
            .map_err(map_octocrab_error)?;

        let status = response.status();
        let headers = response.headers().clone();
        let (remaining, reset, retry_after) = rate_headers(&headers);
        self.remember_rate_headers(remaining, reset, retry_after);
        self.note_spend(&format!("GET {path}"), status, &headers, None);

        if status == StatusCode::NOT_MODIFIED {
            return Ok(Fetched {
                status,
                headers,
                body: String::new(),
            });
        }

        let body = body_text(response).await;
        if !status.is_success() {
            let error = classify(status, remaining, reset, retry_after, body);
            self.remember_rate_limit(&error);
            return Err(error);
        }
        Ok(Fetched {
            status,
            headers,
            body,
        })
    }

    /// One GraphQL request with the same shared rate gate and HTTP error
    /// classification as the REST client. An answer with errors in it is
    /// an error, whatever data came beside them.
    async fn graphql<T: serde::de::DeserializeOwned>(
        &self,
        payload: &serde_json::Value,
        what: &str,
    ) -> Result<T, Error> {
        let (data, problems) = self.graphql_partial(payload, what).await?;
        if !problems.is_empty() {
            return Err(Error::Graphql(problems.join("; ")));
        }
        data.ok_or_else(|| Error::Decode("GitHub's GraphQL response had no data".into()))
    }

    /// One GraphQL request whose answer may be partial: the data that
    /// came, and the errors GitHub reported beside it, for a query whose
    /// parts can fail one at a time. Rate limits and HTTP failures are
    /// errors as everywhere.
    async fn graphql_partial<T: serde::de::DeserializeOwned>(
        &self,
        payload: &serde_json::Value,
        what: &str,
    ) -> Result<(Option<T>, Vec<String>), Error> {
        let payload = if spend_logged() {
            asking_cost(payload)
        } else {
            Cow::Borrowed(payload)
        };
        let asked = matches!(payload, Cow::Owned(_));
        let _request_guard = self.api_permit().await;
        self.check_rate_limit()?;
        let response = self
            .octo
            ._post("/graphql", Some(payload.as_ref()))
            .await
            .map_err(map_octocrab_error)?;
        let what = format!("POST /graphql {what}");
        let body = self
            .finish_costed(response, &[StatusCode::OK], &what, graphql_cost)
            .await?;
        let envelope: GraphqlEnvelope<T> = if asked {
            // Some answers are maps of aliases, which would read the cost
            // as one of theirs.
            let mut value: serde_json::Value =
                serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))?;
            if let Some(data) = value.get_mut("data").and_then(|d| d.as_object_mut()) {
                data.remove("perch_cost");
            }
            serde_json::from_value(value)
        } else {
            serde_json::from_str(&body)
        }
        .map_err(|e| Error::Decode(e.to_string()))?;
        Ok((
            envelope.data,
            envelope
                .errors
                .into_iter()
                .map(|error| error.message)
                .collect(),
        ))
    }

    /// Classify a mutation response with the same error behavior as GETs.
    /// Some notification endpoints use 304 as a
    /// successful idempotent result, so accepted statuses are explicit.
    async fn finish_mutation<B>(
        &self,
        response: http::Response<B>,
        accepted: &[StatusCode],
        what: &str,
    ) -> Result<(), Error>
    where
        B: http_body::Body<Data = Bytes>,
    {
        self.finish_response(response, accepted, what).await.map(drop)
    }

    /// Consume a response, preserving its body for a JSON-returning mutation.
    /// `what` names the request in the spend log.
    async fn finish_response<B>(
        &self,
        response: http::Response<B>,
        accepted: &[StatusCode],
        what: &str,
    ) -> Result<String, Error>
    where
        B: http_body::Body<Data = Bytes>,
    {
        self.finish_costed(response, accepted, what, |_| None).await
    }

    /// The same, with the request's cost read from its body when the body
    /// says it.
    async fn finish_costed<B>(
        &self,
        response: http::Response<B>,
        accepted: &[StatusCode],
        what: &str,
        cost: fn(&str) -> Option<i64>,
    ) -> Result<String, Error>
    where
        B: http_body::Body<Data = Bytes>,
    {
        let status = response.status();
        let headers = response.headers().clone();
        let (remaining, reset, retry_after) = rate_headers(&headers);
        self.remember_rate_headers(remaining, reset, retry_after);
        let body = body_text(response).await;
        if spend_logged() {
            self.note_spend(what, status, &headers, cost(&body));
        }

        if accepted.contains(&status) {
            return Ok(body);
        }
        let error = classify(status, remaining, reset, retry_after, body);
        self.remember_rate_limit(&error);
        Err(error)
    }

    /// A place among the requests in flight, held until the response is
    /// read.
    async fn api_permit(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.api_requests
            .acquire()
            .await
            .expect("the API semaphore is never closed")
    }

    fn check_rate_limit(&self) -> Result<(), Error> {
        let until = self.rate_limit_until.load(Ordering::SeqCst);
        if until > Timestamp::now().as_second() {
            return Err(Error::RateLimited {
                reset_epoch: Some(until),
                retry_after_secs: None,
            });
        }
        Ok(())
    }

    fn remember_rate_headers(
        &self,
        remaining: Option<i64>,
        reset_epoch: Option<i64>,
        retry_after_secs: Option<u64>,
    ) {
        if remaining == Some(0) || retry_after_secs.is_some() {
            self.remember_rate_limit(&Error::RateLimited {
                reset_epoch,
                retry_after_secs,
            });
        }
    }

    fn remember_rate_limit(&self, error: &Error) {
        let Error::RateLimited {
            reset_epoch,
            retry_after_secs,
        } = error
        else {
            return;
        };
        let until = rate_limit_deadline(*reset_epoch, *retry_after_secs);
        self.rate_limit_until.fetch_max(until, Ordering::SeqCst);
    }

    /// Log one request's cost against its rate-limit resource, beside what
    /// this process has spent of that resource in the current window and
    /// since launch. `used` is the whole account's spend in the window,
    /// which counts every other client on the same token too.
    ///
    /// `cost` is what the response said it cost. Without it, a request
    /// costs one, except a 304, which GitHub does not count.
    fn note_spend(&self, what: &str, status: StatusCode, headers: &HeaderMap, cost: Option<i64>) {
        if !spend_logged() {
            return;
        }
        let resource =
            header_str(headers, "x-ratelimit-resource").unwrap_or_else(|| "unknown".into());
        let reset = header_str(headers, "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
        let cost = cost.unwrap_or(i64::from(status != StatusCode::NOT_MODIFIED));
        let now = Timestamp::now().as_second();
        let (window, total) = {
            let mut spent = self.spent.lock().unwrap();
            let spent = spent.entry(resource.clone()).or_default();
            // Windows an hour past their reset are done with.
            spent.windows.retain(|reset, _| *reset > now - 3600);
            let window = spent.windows.entry(reset.unwrap_or(0)).or_default();
            *window += cost;
            let window = *window;
            spent.total += cost;
            (window, spent.total)
        };
        let header = |name| header_str(headers, name).unwrap_or_else(|| "?".into());
        tracing::debug!(
            target: SPEND,
            cost,
            window,
            total,
            used = %format!("{}/{}", header("x-ratelimit-used"), header("x-ratelimit-limit")),
            resets_in = %reset.map_or("?".into(), |reset| format!("{}s", reset - now)),
            "{what} {} {resource}",
            status.as_u16(),
        );
    }

    /// The repository's latest release: its newest that is neither a
    /// draft nor a prerelease. `None` when it has none, or when the token
    /// can't see the repository, since GitHub answers both with a 404.
    pub async fn latest_release(&self, repo_full_name: &str) -> Result<Option<Release>, Error> {
        let body = match self
            .get_text(&format!("/repos/{repo_full_name}/releases/latest"))
            .await
        {
            Ok(body) => body,
            Err(Error::Status { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        serde_json::from_str(&body)
            .map(Some)
            .map_err(|e| Error::Decode(e.to_string()))
    }

    /// Download a release's file, whole, into memory.
    ///
    /// It's asked of the API, with the token when that's api.github.com,
    /// which redirects to storage elsewhere; the client drops the token on
    /// the way. Anything short of the size the release lists is an error,
    /// so a dropped connection never passes for the file.
    pub async fn download_release_asset(&self, asset: &ReleaseAsset) -> Result<Vec<u8>, Error> {
        let mut request = self
            .http
            .get(&asset.url)
            .header(http::header::ACCEPT, "application/octet-stream")
            .timeout(RELEASE_ASSET_TIMEOUT);
        if asset.url.starts_with("https://api.github.com/") {
            request = request.bearer_auth(&*self.token);
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(Error::Status {
                status: response.status().as_u16(),
                body: format!("download of {} failed", asset.name),
            });
        }
        let mut bytes = Vec::with_capacity(asset.size as usize);
        while let Some(chunk) = response.chunk().await.map_err(|e| Error::Network(e.to_string()))? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > asset.size {
                break;
            }
        }
        if bytes.len() as u64 != asset.size {
            return Err(Error::Network(format!(
                "{} came to {} bytes, not the {} its release lists",
                asset.name,
                bytes.len(),
                asset.size
            )));
        }
        Ok(bytes)
    }

    /// Download an avatar at display size. Returns raw encoded bytes.
    pub async fn fetch_avatar(&self, url: &str) -> Result<Vec<u8>, Error> {
        let sized = with_size_param(url, 64);
        let response = self
            .http
            .get(&sized)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::Status {
                status: response.status().as_u16(),
                body: "avatar fetch failed".into(),
            });
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| Error::Network(e.to_string()))
    }

    /// Download a picture a comment shows. Returns raw encoded bytes.
    ///
    /// One uploaded to github.com, as GitHub's editor puts a pasted
    /// screenshot at `github.com/user-attachments/assets/…`, is asked for
    /// with the token, which a private repository's needs; it redirects to
    /// storage elsewhere, and the client drops the token on the way. Any
    /// other host is asked without it.
    pub async fn fetch_picture(&self, url: &str) -> Result<Vec<u8>, Error> {
        let mut request = self.http.get(url);
        if sends_token(url) {
            request = request.bearer_auth(&*self.token);
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(Error::Status {
                status: response.status().as_u16(),
                body: "picture fetch failed".into(),
            });
        }
        if response.content_length().is_some_and(|length| length > PICTURE_BYTES) {
            return Err(Error::Decode(format!("picture over {PICTURE_BYTES} bytes")));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| Error::Network(e.to_string()))? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > PICTURE_BYTES {
                return Err(Error::Decode(format!("picture over {PICTURE_BYTES} bytes")));
            }
        }
        Ok(bytes)
    }
}

/// How long a release's file may take to download. The client's own 20
/// seconds is for API calls; Perch's tarball is tens of megabytes.
const RELEASE_ASSET_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The most a picture may weigh: GitHub takes uploads of up to 10 MB, and
/// a picture from elsewhere bigger than this is not one to draw in a
/// comment.
const PICTURE_BYTES: u64 = 25 * 1024 * 1024;

/// Whether a picture's address is on github.com over https, the one host
/// the token goes to with it.
fn sends_token(url: &str) -> bool {
    url.strip_prefix("https://github.com/").is_some()
}

#[derive(Debug, serde::Deserialize)]
struct GraphqlEnvelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphqlProblem>,
}

#[derive(Debug, serde::Deserialize)]
struct GraphqlProblem {
    #[serde(default)]
    message: String,
}

/// Whether the spend log is on.
fn spend_logged() -> bool {
    tracing::enabled!(target: SPEND, tracing::Level::DEBUG)
}

/// The payload with its query asking its own cost beside its answer. A
/// mutation is left as it is: `rateLimit` is a field of `Query`.
fn asking_cost(payload: &serde_json::Value) -> Cow<'_, serde_json::Value> {
    let Some(query) = payload.get("query").and_then(serde_json::Value::as_str) else {
        return Cow::Borrowed(payload);
    };
    let open = query.find('{');
    let Some(open) = open.filter(|_| !query.trim_start().starts_with("mutation")) else {
        return Cow::Borrowed(payload);
    };
    let mut asked = payload.clone();
    asked["query"] = format!("{} {COST_FIELD}{}", &query[..=open], &query[open + 1..]).into();
    Cow::Owned(asked)
}

/// What a GraphQL answer says its query cost, when it was asked.
fn graphql_cost(body: &str) -> Option<i64> {
    #[derive(serde::Deserialize)]
    struct Probe {
        data: Option<Data>,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        perch_cost: Option<Cost>,
    }
    #[derive(serde::Deserialize)]
    struct Cost {
        cost: i64,
    }
    let probe: Probe = serde_json::from_str(body).ok()?;
    Some(probe.data?.perch_cost?.cost)
}

/// Split a repository identity before placing it in either a GraphQL
/// variable or REST path.
fn repo_parts(repo_full_name: &str) -> Result<(&str, &str), Error> {
    match repo_full_name.split_once('/') {
        Some((owner, name)) if !owner.is_empty() && !name.is_empty() && !name.contains('/') => {
            Ok((owner, name))
        }
        _ => Err(Error::Decode(format!(
            "repository name is not owner/repo: {repo_full_name}"
        ))),
    }
}

/// The request headers for a conditional GET: `If-None-Match` when there
/// is a tag to ask with, nothing otherwise.
fn conditional(etag: Option<&str>) -> Option<HeaderMap> {
    let value = etag.and_then(|e| HeaderValue::from_str(e).ok())?;
    let mut headers = HeaderMap::new();
    headers.insert(IF_NONE_MATCH, value);
    Some(headers)
}

/// One page of a list endpoint, a hundred to the page.
fn page_path(path: &str, page: usize) -> String {
    let joiner = if path.contains('?') { '&' } else { '?' };
    format!("{path}{joiner}per_page={LIST_PER_PAGE}&page={page}")
}

/// The page a `Link` header names as the last, as GitHub writes one:
/// `<…&page=2>; rel="next", <…&page=12>; rel="last"`.
fn last_page(link: &str) -> Option<usize> {
    link.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        let last = params
            .split(';')
            .any(|param| param.trim().replace(' ', "") == r#"rel="last""#);
        if !last {
            return None;
        }
        let url = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        let (_, query) = url.split_once('?')?;
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("page="))
            .and_then(|page| page.parse().ok())
    })
}

/// The page to read a list from for its `newest` pages after the first,
/// when its last page is `last`. Without a last page, every page after
/// the first is read.
fn newest_pages_from(last: Option<usize>, newest: usize) -> usize {
    last.map_or(2, |last| (last + 1).saturating_sub(newest).max(2))
}

/// The conditional header for a notifications probe.
fn modified_since(last_modified: Option<&str>) -> Option<HeaderMap> {
    let value = last_modified.and_then(|value| HeaderValue::from_str(value).ok())?;
    let mut headers = HeaderMap::new();
    headers.insert(IF_MODIFIED_SINCE, value);
    Some(headers)
}

/// Build a notification endpoint from the numeric ID GitHub supplied.
fn notification_thread_path(thread_id: &str, suffix: &str) -> Result<String, Error> {
    if thread_id.is_empty() || !thread_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::Decode(format!(
            "notification thread id is not numeric: {thread_id}"
        )));
    }
    Ok(format!("/notifications/threads/{thread_id}{suffix}"))
}

/// GitHub serves avatars at whatever size `s` asks for. Asking for 64 keeps a
/// 20pt circle crisp on a retina display without pulling the full image.
fn with_size_param(url: &str, size: u32) -> String {
    if url.contains('?') {
        format!("{url}&s={size}")
    } else {
        format!("{url}?s={size}")
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

/// Requests remaining, the primary-window reset epoch, and a secondary-limit
/// delay. Any value is `None` when its header is absent or unreadable.
/// The query without its `sort:` qualifier, and the sort and order it
/// asked for. The API reads the order from parameters, not the query;
/// without a qualifier, or with one it does not know, it is most recently
/// updated first.
fn search_sort(query: &str) -> (String, &'static str, &'static str) {
    let mut sort = ("updated", "desc");
    let rest: Vec<&str> = query
        .split_whitespace()
        .filter(|term| {
            let Some(value) = term.strip_prefix("sort:") else {
                return true;
            };
            let (field, order) = value.rsplit_once('-').unwrap_or((value, "desc"));
            let field = match field {
                "updated" => "updated",
                "created" => "created",
                "comments" => "comments",
                _ => return false,
            };
            let order = if order == "asc" { "asc" } else { "desc" };
            sort = (field, order);
            false
        })
        .collect();
    (rest.join(" "), sort.0, sort.1)
}

fn rate_headers(headers: &HeaderMap) -> (Option<i64>, Option<i64>, Option<u64>) {
    let remaining =
        header_str(headers, "x-ratelimit-remaining").and_then(|v| v.parse::<i64>().ok());
    let reset = header_str(headers, "x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
    let retry_after = header_str(headers, "retry-after").and_then(|v| v.parse::<u64>().ok());
    (remaining, reset, retry_after)
}

fn rate_limit_deadline(reset_epoch: Option<i64>, retry_after_secs: Option<u64>) -> i64 {
    let now = Timestamp::now().as_second();
    if let Some(seconds) = retry_after_secs {
        let seconds = i64::try_from(seconds.max(1)).unwrap_or(i64::MAX);
        return now.saturating_add(seconds);
    }
    reset_epoch
        .filter(|reset| *reset > now)
        .unwrap_or_else(|| now.saturating_add(60))
}

async fn body_text(response: http::Response<impl http_body::Body<Data = Bytes>>) -> String {
    match response.into_body().collect().await {
        Ok(collected) => String::from_utf8_lossy(&collected.to_bytes()).into_owned(),
        Err(_) => String::new(),
    }
}

fn classify(
    status: StatusCode,
    rate_remaining: Option<i64>,
    rate_reset: Option<i64>,
    retry_after_secs: Option<u64>,
    body: String,
) -> Error {
    if status == StatusCode::UNAUTHORIZED {
        return Error::Unauthorized;
    }
    // A spent window leaves `remaining` at zero. A secondary limit does not,
    // and says so only in the body, so the body is read for it too.
    let exhausted = rate_remaining == Some(0) || body.to_ascii_lowercase().contains("rate limit");
    if (status == StatusCode::FORBIDDEN && exhausted) || status == StatusCode::TOO_MANY_REQUESTS {
        return Error::RateLimited {
            reset_epoch: rate_reset,
            retry_after_secs,
        };
    }
    Error::Status {
        status: status.as_u16(),
        body: body.chars().take(300).collect(),
    }
}

fn map_octocrab_error(e: octocrab::Error) -> Error {
    match &e {
        octocrab::Error::GitHub { source, .. }
            if source.status_code == StatusCode::UNAUTHORIZED =>
        {
            Error::Unauthorized
        }
        _ => Error::Network(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sort_qualifier_becomes_the_search_s_parameters() {
        assert_eq!(
            search_sort("author:@me is:open archived:false"),
            ("author:@me is:open archived:false".to_string(), "updated", "desc")
        );
        assert_eq!(
            search_sort("author:@me sort:created-asc is:open"),
            ("author:@me is:open".to_string(), "created", "asc")
        );
        assert_eq!(search_sort("repo:a/b sort:updated"), ("repo:a/b".to_string(), "updated", "desc"));
        assert_eq!(search_sort("repo:a/b sort:bogus-asc"), ("repo:a/b".to_string(), "updated", "desc"));
    }

    #[test]
    fn a_link_header_says_which_page_is_the_last() {
        let link = r#"<https://api.github.com/repositories/1/issues/7/timeline?per_page=100&page=2>; rel="next", <https://api.github.com/repositories/1/issues/7/timeline?per_page=100&page=25>; rel="last""#;
        assert_eq!(last_page(link), Some(25));
        let on_the_last = r#"<https://x/t?page=1&per_page=100>; rel="first", <https://x/t?page=24&per_page=100>; rel="prev""#;
        assert_eq!(last_page(on_the_last), None, "the last page names no last");
        assert_eq!(last_page(""), None);
        assert_eq!(last_page(r#"<https://x/t?per_page=100>; rel="last""#), None);
    }

    #[test]
    fn a_long_list_is_read_from_its_newest_pages() {
        // Nine newest pages of twenty-five are 17 to 25.
        assert_eq!(newest_pages_from(Some(25), 9), 17);
        // Ten pages in all are read whole after the first.
        assert_eq!(newest_pages_from(Some(10), 9), 2);
        assert_eq!(newest_pages_from(Some(3), 9), 2);
        assert_eq!(newest_pages_from(None, 9), 2);
        assert_eq!(newest_pages_from(Some(25), 1), 25);
    }

    #[test]
    fn avatar_urls_gain_a_size_parameter() {
        assert_eq!(
            with_size_param("https://x/a.png", 64),
            "https://x/a.png?s=64"
        );
        assert_eq!(
            with_size_param("https://x/a?v=4", 64),
            "https://x/a?v=4&s=64"
        );
    }

    #[test]
    fn only_a_picture_on_github_com_is_asked_for_with_the_token() {
        assert!(sends_token("https://github.com/user-attachments/assets/0a1b"));
        assert!(sends_token("https://github.com/o/r/assets/1/0a1b"));
        assert!(!sends_token("http://github.com/user-attachments/assets/0a1b"));
        assert!(!sends_token("https://github.com.evil.example/x.png"));
        assert!(!sends_token("https://user-images.githubusercontent.com/1/x.png"));
        assert!(!sends_token("https://example.com/https://github.com/x.png"));
    }

    #[test]
    fn unauthorized_is_distinct_from_rate_limiting() {
        assert!(matches!(
            classify(StatusCode::UNAUTHORIZED, None, None, None, String::new()),
            Error::Unauthorized
        ));
        assert!(matches!(
            classify(
                StatusCode::FORBIDDEN,
                Some(0),
                Some(123),
                None,
                String::new()
            ),
            Error::RateLimited {
                reset_epoch: Some(123),
                retry_after_secs: None
            }
        ));
        // A 403 that is not a rate limit stays a plain status error.
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, Some(42), None, None, "no".into()),
            Error::Status { status: 403, .. }
        ));
    }

    #[test]
    fn a_403_that_names_the_rate_limit_is_rate_limited_whatever_remains() {
        let body = r#"{"message":"You have exceeded a secondary rate limit."}"#.to_string();
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, Some(42), Some(9), Some(120), body),
            Error::RateLimited {
                reset_epoch: Some(9),
                retry_after_secs: Some(120)
            }
        ));
    }

    #[test]
    fn a_query_asks_its_cost_and_a_mutation_does_not() {
        let query = serde_json::json!({
            "query": "\nquery Q($a: Int!) {\n  repository { id }\n}\nfragment F on X { id }",
            "variables": { "a": 1 },
        });
        let asked = asking_cost(&query);
        assert_eq!(
            asked["query"],
            "\nquery Q($a: Int!) { perch_cost: rateLimit { cost }\n  repository { id }\n}\nfragment F on X { id }"
        );
        assert_eq!(asked["variables"], query["variables"]);

        let mutation = serde_json::json!({ "query": "mutation($s: ID!) { addReaction { id } }" });
        assert!(matches!(asking_cost(&mutation), Cow::Borrowed(_)));
    }

    #[test]
    fn a_graphql_answer_says_what_it_cost() {
        assert_eq!(
            graphql_cost(r#"{"data":{"r0":{},"perch_cost":{"cost":3}}}"#),
            Some(3)
        );
        assert_eq!(graphql_cost(r#"{"data":{"r0":{}}}"#), None);
        assert_eq!(graphql_cost(r#"{"errors":[]}"#), None);
    }

    #[tokio::test]
    async fn retry_after_sets_a_fail_fast_shared_gate_and_wins_over_reset() {
        let gh = GitHub::new("token").unwrap();
        let before = Timestamp::now().as_second();
        gh.remember_rate_limit(&Error::RateLimited {
            reset_epoch: Some(before + 60),
            retry_after_secs: Some(3_600),
        });
        assert!(gh.rate_limit_until.load(Ordering::SeqCst) >= before + 3_600);
        assert!(matches!(
            gh.check_rate_limit(),
            Err(Error::RateLimited {
                reset_epoch: Some(reset),
                retry_after_secs: None
            }) if reset >= before + 3_600
        ));
    }
}
