//! Repository-wide GitHub reads for Perch's Markdown archive.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{repo_parts, Error, GitHub, LIST_PER_PAGE};
use crate::discussion::{
    Discussion, IssueCommentPayload, PullSummary, ReviewCommentPayload, ReviewPayload,
};
use crate::items::{IssuePayload, Subject, SubjectKind};
use crate::pulls::PullPayload;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryStream {
    /// The issues endpoint lists both issues and pull requests.
    Items,
    /// Conversation comments on issues and pull requests.
    IssueComments,
    /// Inline comments on pull requests.
    ReviewComments,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct HistorySubject {
    pub kind: SubjectKind,
    pub number: u64,
}

#[derive(Debug)]
pub struct HistoryPage {
    pub subjects: Vec<HistorySubject>,
    pub next_path: Option<String>,
}

#[derive(Deserialize)]
struct RepoIdentity {
    full_name: String,
}

#[derive(Deserialize)]
struct ItemEntry {
    number: u64,
    #[serde(default)]
    pull_request: Option<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct IssueCommentEntry {
    html_url: String,
}

#[derive(Deserialize)]
struct ReviewCommentEntry {
    pull_request_url: String,
}

impl GitHub {
    /// A page of repository history, starting at page one. `since` filters
    /// by last update. Keep creation order so an edit does not move an
    /// existing result across page boundaries. Comments can be edited
    /// independently of the parent item's `updated_at`.
    pub async fn history_page(
        &self,
        repo: &str,
        stream: HistoryStream,
        since: Option<Timestamp>,
        next_path: Option<&str>,
    ) -> Result<HistoryPage, Error> {
        repo_parts(repo)?;
        let _read = self.history_read.lock().await;
        let path = match next_path {
            Some(path) => path.to_owned(),
            None => {
                let endpoint = match stream {
                    HistoryStream::Items => "issues?state=all&sort=created&direction=asc",
                    HistoryStream::IssueComments => "issues/comments?sort=created&direction=asc",
                    HistoryStream::ReviewComments => "pulls/comments?sort=created&direction=asc",
                };
                let mut path = format!("/repos/{repo}/{endpoint}&per_page={LIST_PER_PAGE}");
                if let Some(since) = since {
                    let since = Timestamp::from_second(since.as_second())
                        .map_err(|error| Error::Decode(error.to_string()))?;
                    path.push_str(&format!("&since={since}"));
                }
                path
            }
        };
        let (body, next_path) = self.for_history().history_rest_page(&path).await?;
        let subjects = match stream {
            HistoryStream::Items => decode::<Vec<ItemEntry>>(&body)?
                .into_iter()
                .map(|item| HistorySubject {
                    kind: if item.pull_request.is_some() {
                        SubjectKind::PullRequest
                    } else {
                        SubjectKind::Issue
                    },
                    number: item.number,
                })
                .collect(),
            HistoryStream::IssueComments => decode::<Vec<IssueCommentEntry>>(&body)?
                .into_iter()
                .map(|comment| comment_subject(&comment.html_url))
                .collect::<Result<Vec<_>, _>>()?,
            HistoryStream::ReviewComments => decode::<Vec<ReviewCommentEntry>>(&body)?
                .into_iter()
                .map(|comment| comment_subject(&comment.pull_request_url))
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(HistoryPage {
            subjects,
            next_path,
        })
    }

    /// Resolve a configured name before importing. GitHub may redirect a
    /// renamed repository even while Perch still grants its configured name.
    pub async fn history_repo_name(&self, repo: &str) -> Result<String, Error> {
        repo_parts(repo)?;
        let _read = self.history_read.lock().await;
        self.for_history().read_history_repo_name(repo).await
    }

    /// Read the description and submitted conversation for an archive entry.
    /// None means GitHub marked the subject gone or moved it elsewhere.
    /// An ambiguous 404 never authorizes deleting a cached conversation.
    pub async fn fetch_history_subject(
        &self,
        repo: &str,
        canonical_repo: &str,
        subject: HistorySubject,
    ) -> Result<Option<(Subject, Discussion)>, Error> {
        repo_parts(repo)?;
        let _read = self.history_read.lock().await;
        let github = self.for_history();
        let number = subject.number;
        let base = format!("/repos/{repo}");
        if subject.kind == SubjectKind::Issue {
            let Some(body) = github.history_item_body(repo, "issues", number).await? else {
                return Ok(None);
            };
            let payload: IssuePayload = decode(&body)?;
            if payload.number != number {
                return Ok(None);
            }
            if let Some(url) = &payload.repository_url {
                let origin = repository_name_from_url(url)?;
                if !origin.eq_ignore_ascii_case(canonical_repo)
                    && !origin.eq_ignore_ascii_case(&github.read_history_repo_name(repo).await?)
                {
                    return Ok(None);
                }
            }
            // The listing or a deferred checkpoint can have an older kind.
            if !payload.is_pull_request() {
                let issue = payload.into_issue(repo);
                let comments = github
                    .history_list::<IssueCommentPayload>(&format!(
                        "{base}/issues/{number}/comments"
                    ))
                    .await?;
                let discussion = Discussion {
                    node_id: issue.node_id.clone(),
                    opened_at: issue.created_at,
                    body: issue.body.clone(),
                    comments: comments.into_iter().map(Into::into).collect(),
                    ..Discussion::default()
                };
                return Ok(Some((Subject::Issue(issue), discussion)));
            }
        }

        let Some(body) = github.history_item_body(repo, "pulls", number).await? else {
            return Ok(None);
        };
        let mut pull = decode::<PullPayload>(&body)?.into_pull_request(repo);
        if pull.number != number {
            return Ok(None);
        }
        // GitHub returns the base repository's current name after a rename.
        // Keep the configured identity for both kinds of archive entry.
        pull.repo_full_name = repo.to_string();
        let summary: PullSummary = decode(&body)?;
        let comments = github
            .history_list(&format!("{base}/issues/{number}/comments"))
            .await?;
        let mut reviews: Vec<ReviewPayload> = github
            .history_list(&format!("{base}/pulls/{number}/reviews"))
            .await?;
        let mut review_comments: Vec<ReviewCommentPayload> = github
            .history_list(&format!("{base}/pulls/{number}/comments"))
            .await?;

        let pending: HashSet<u64> = reviews
            .iter()
            .filter(|review| review.state == "PENDING")
            .map(|review| review.id)
            .collect();
        reviews.retain(|review| !pending.contains(&review.id));
        review_comments.retain(|comment| {
            !comment
                .pull_request_review_id
                .is_some_and(|id| pending.contains(&id))
        });

        let mut discussion = summary.into_discussion(comments, reviews, review_comments);
        if !discussion.review_comments.is_empty() {
            match github
                .fetch_review_thread_states(canonical_repo, number)
                .await
            {
                Ok(states) => discussion.review_threads = states,
                Err(error @ Error::RateLimited { .. }) => return Err(error),
                Err(error) => {
                    tracing::debug!(repo, number, %error, "could not read review thread states for history")
                }
            }
        }
        Ok(Some((Subject::PullRequest(pull), discussion)))
    }

    async fn read_history_repo_name(&self, repo: &str) -> Result<String, Error> {
        let body = self.get_text(&format!("/repos/{repo}")).await?;
        let identity: RepoIdentity = decode(&body)?;
        repo_parts(&identity.full_name)?;
        Ok(identity.full_name)
    }

    async fn history_item_body(
        &self,
        repo: &str,
        endpoint: &str,
        number: u64,
    ) -> Result<Option<String>, Error> {
        match self
            .get_text(&format!("/repos/{repo}/{endpoint}/{number}"))
            .await
        {
            Ok(body) => Ok(Some(body)),
            Err(Error::Status { status: 410, .. }) => Ok(None),
            Err(Error::Status { status: 404, .. }) => {
                // Private resources also return 404 after a permission
                // change. Stop the batch if the repository cannot be read.
                self.read_history_repo_name(repo).await?;
                Err(Error::HistorySubjectUnavailable)
            }
            Err(error) => Err(error),
        }
    }

    async fn history_list<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
    ) -> Result<Vec<T>, Error> {
        let mut page = Some(format!("{endpoint}?per_page={LIST_PER_PAGE}"));
        let mut all = Vec::new();
        while let Some(path) = page {
            let (body, next) = self
                .history_rest_page(&path)
                .await
                .map_err(conversation_error)?;
            all.extend(decode::<Vec<T>>(&body)?);
            page = next;
        }
        Ok(all)
    }

    async fn history_rest_page(&self, path: &str) -> Result<(String, Option<String>), Error> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(Error::Decode(
                "invalid GitHub history pagination path".into(),
            ));
        }
        let (body, link) = self.get_linked(path).await?;
        let next = next_history_path(link.as_deref())?;
        if next.as_deref() == Some(path) {
            return Err(Error::Decode(
                "GitHub returned the same history page twice".into(),
            ));
        }
        Ok((body, next))
    }

    /// When history can resume, as seconds since the epoch. History leaves
    /// a reserve for interactive reads and writes on the same token.
    pub fn history_rate_limited_until(&self) -> Option<i64> {
        let until = self
            .history_rate_limit_until
            .load(Ordering::SeqCst)
            .max(self.rate_limit_until.load(Ordering::SeqCst));
        (until > Timestamp::now().as_second()).then_some(until)
    }

    fn for_history(&self) -> Self {
        Self {
            history_request: true,
            ..self.clone()
        }
    }
}

fn conversation_error(error: Error) -> Error {
    match error {
        Error::Status {
            status: 404 | 410, ..
        } => Error::HistorySubjectUnavailable,
        error => error,
    }
}

fn next_history_path(link: Option<&str>) -> Result<Option<String>, Error> {
    for part in link.into_iter().flat_map(|link| link.split(',')) {
        let Some((target, params)) = part.split_once(';') else {
            continue;
        };
        if params
            .split(';')
            .any(|param| param.trim() == r#"rel="next""#)
        {
            let url = target
                .trim()
                .strip_prefix('<')
                .and_then(|target| target.strip_suffix('>'))
                .and_then(|target| reqwest::Url::parse(target).ok())
                .filter(|url| matches!(url.scheme(), "http" | "https"))
                .ok_or_else(|| Error::Decode("invalid GitHub history next link".into()))?;
            // Only send the path and query to the configured GitHub origin.
            // GitHub may use a numeric repository path in its link.
            let path = match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_owned(),
            };
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn repository_name_from_url(url: &str) -> Result<String, Error> {
    let url = reqwest::Url::parse(url)
        .map_err(|error| Error::Decode(format!("invalid issue repository URL: {error}")))?;
    let repo = url
        .path()
        .strip_prefix("/repos/")
        .ok_or_else(|| Error::Decode("issue repository URL has no repository name".into()))?;
    repo_parts(repo)?;
    Ok(repo.to_owned())
}

fn decode<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, Error> {
    serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))
}

fn comment_subject(url: &str) -> Result<HistorySubject, Error> {
    let parse = || {
        let path = url.split('#').next()?;
        let (parent, number) = path.rsplit_once('/')?;
        let kind = match parent.rsplit('/').next()? {
            "pull" | "pulls" => SubjectKind::PullRequest,
            "issues" => SubjectKind::Issue,
            _ => return None,
        };
        Some(HistorySubject {
            kind,
            number: number.parse().ok()?,
        })
    };
    parse().ok_or_else(|| Error::Decode(format!("unexpected GitHub comment subject URL: {url}")))
}

#[cfg(test)]
mod tests {
    use http::HeaderMap;

    use super::*;
    use crate::client::HISTORY_RATE_RESERVE;

    #[tokio::test]
    async fn the_history_reserve_is_shared_and_leaves_interactive_reads_available() {
        let github = GitHub::new("token").unwrap();
        let reset = Timestamp::now().as_second() + 60;
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-resource", "core".parse().unwrap());
        github.clone().remember_rate_headers(
            &headers,
            Some(HISTORY_RATE_RESERVE),
            Some(reset),
            None,
        );

        assert_eq!(github.history_rate_limited_until(), Some(reset));
        assert!(matches!(
            github.for_history().check_rate_limit(),
            Err(Error::RateLimited { reset_epoch: Some(epoch), .. }) if epoch == reset
        ));
        assert!(github.check_rate_limit().is_ok());
    }
}
