//! What Open… and the Browse search box take: a pull request's or an
//! issue's URL, `owner/repo#57`, or a bare `#57`.
//!
//! Pure, so the box, the search row and the launch hooks read a reference
//! the same way.

use perch_github::ItemKind;

/// A pull request or an issue, as named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// `owner/repo`, or `None` for a bare number, which takes its
    /// repository from where the reader is.
    pub repo: Option<String>,
    pub number: u64,
    /// Known from a URL. A reference doesn't say, and GitHub is asked.
    pub kind: Option<ItemKind>,
}

/// Read `text` as a reference. A URL counts on `github.com` and on any of
/// `hosts`, the configured GitHub hosts; anything after the number in one
/// is ignored.
pub fn parse(text: &str, hosts: &[String]) -> Option<Reference> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(reference) = url(text, hosts) {
        return Some(reference);
    }
    if let Some((repo, number)) = text.split_once('#') {
        let number = positive(number)?;
        let repo = repo.trim();
        if repo.is_empty() {
            return Some(Reference {
                repo: None,
                number,
                kind: None,
            });
        }
        return is_repo(repo).then(|| Reference {
            repo: Some(repo.to_string()),
            number,
            kind: None,
        });
    }
    positive(text).map(|number| Reference {
        repo: None,
        number,
        kind: None,
    })
}

/// `https://<host>/<owner>/<repo>/pull/<n>` or `/issues/<n>`, with or
/// without the scheme.
fn url(text: &str, hosts: &[String]) -> Option<Reference> {
    let lower = text.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| lower.starts_with(scheme).then(|| &text[scheme.len()..]))
        .unwrap_or(text);
    let (host, path) = rest.split_once('/')?;
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let known = host == "github.com" || hosts.iter().any(|h| h.eq_ignore_ascii_case(host));
    if !known {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    let (owner, repo, what, number) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let kind = match what {
        "pull" | "pulls" => ItemKind::PullRequest,
        "issues" => ItemKind::Issue,
        _ => return None,
    };
    Some(Reference {
        repo: Some(format!("{owner}/{repo}")),
        number: positive(number)?,
        kind: Some(kind),
    })
}

/// A launch hook's `owner/repo#N`, or a URL to one on github.com.
pub fn launch_hook(spec: &str) -> Option<(String, u64)> {
    let reference = parse(spec, &[])?;
    Some((reference.repo?, reference.number))
}

fn positive(digits: &str) -> Option<u64> {
    let digits = digits.trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|n| *n > 0)
}

/// `owner/repo`: two non-empty parts, no spaces.
fn is_repo(text: &str) -> bool {
    match text.split_once('/') {
        Some((owner, repo)) => {
            !owner.is_empty()
                && !repo.is_empty()
                && !repo.contains('/')
                && !text.contains(char::is_whitespace)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(repo: &str, number: u64, kind: Option<ItemKind>) -> Option<Reference> {
        Some(Reference {
            repo: Some(repo.into()),
            number,
            kind,
        })
    }

    fn bare(number: u64) -> Option<Reference> {
        Some(Reference {
            repo: None,
            number,
            kind: None,
        })
    }

    #[test]
    fn urls_on_github_say_what_they_are() {
        let none: &[String] = &[];
        assert_eq!(
            parse("https://github.com/slatedb/slatedb/pull/57", none),
            named("slatedb/slatedb", 57, Some(ItemKind::PullRequest))
        );
        assert_eq!(
            parse("https://github.com/slatedb/slatedb/issues/9", none),
            named("slatedb/slatedb", 9, Some(ItemKind::Issue))
        );
        assert_eq!(
            parse("  github.com/slatedb/slatedb/pull/57/files  ", none),
            named("slatedb/slatedb", 57, Some(ItemKind::PullRequest)),
            "no scheme, a trailing path"
        );
        assert_eq!(
            parse("https://www.github.com/a/b/issues/3#issuecomment-12", none),
            named("a/b", 3, Some(ItemKind::Issue)),
            "a fragment"
        );
        assert_eq!(
            parse("https://github.com/a/b/pull/4?w=1", none),
            named("a/b", 4, Some(ItemKind::PullRequest)),
            "a query"
        );
        assert_eq!(parse("https://github.com/a/b/commit/4", none), None);
        assert_eq!(parse("https://github.com/a/b", none), None);
        assert_eq!(parse("https://github.com/a/b/pull/x", none), None);
    }

    #[test]
    fn a_configured_host_counts_and_others_do_not() {
        let hosts = vec!["github.com".to_string(), "git.corp.example".to_string()];
        assert_eq!(
            parse("https://git.corp.example/team/app/pull/12", &hosts),
            named("team/app", 12, Some(ItemKind::PullRequest))
        );
        assert_eq!(parse("https://gitlab.com/team/app/pull/12", &hosts), None);
    }

    #[test]
    fn references_and_bare_numbers() {
        let none: &[String] = &[];
        assert_eq!(
            parse("slatedb/slatedb#57", none),
            named("slatedb/slatedb", 57, None)
        );
        assert_eq!(parse("#57", none), bare(57));
        assert_eq!(parse("57", none), bare(57));
        assert_eq!(parse(" # 57 ", none), bare(57));
        assert_eq!(parse("#0", none), None);
        assert_eq!(parse("slatedb#57", none), None, "not owner/repo");
        assert_eq!(parse("a/b/c#57", none), None);
        assert_eq!(parse("a b/c#57", none), None);
        assert_eq!(parse("fence", none), None);
        assert_eq!(parse("", none), None);
    }
}
