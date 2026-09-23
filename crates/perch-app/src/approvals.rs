//! What an approval request is called, what "Always" keeps for it, and
//! whether a kept grant answers it.
//!
//! A grant is Perch's, not the CLI's: it lives in the database keyed by
//! repository and by what it allows, and Perch answers a request it
//! covers the moment the request arrives. The key is a string so the
//! database needs no idea of kinds: `network:<host>`, `command:<text>`,
//! `tool:<name>:<what>`, and `commit`.

use perch_agent::{ApprovalKind, ApprovalRequest};
use perch_github::WorkGrantRecord;

/// How long a request waits for the reader before it is denied.
pub const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// What the agent is told when the reader says no.
pub const DENIED: &str = "The reader said no.";

/// The card's heading for a request.
pub fn title(request: &ApprovalRequest) -> String {
    match &request.kind {
        ApprovalKind::Host => format!("Connect to {}", request.subject),
        ApprovalKind::Command => "Run outside the sandbox".to_string(),
        ApprovalKind::Edit => format!("Edit {}", request.subject),
        ApprovalKind::Tool { name } => format!("Use {name}"),
    }
}

/// The request in one line, for a step and the inspector: the heading,
/// and the command when that is what it is about.
pub fn one_line(request: &ApprovalRequest) -> String {
    match &request.kind {
        ApprovalKind::Command => format!("Run outside the sandbox: {}", request.subject),
        ApprovalKind::Tool { .. } => format!("{}: {}", title(request), request.subject),
        _ => title(request),
    }
}

/// Git subcommands that write the repository, which the commit grant
/// covers.
const GIT_WRITES: &[&str] = &[
    "add", "commit", "stash", "rm", "mv", "restore", "reset", "checkout", "switch", "merge",
    "rebase", "cherry-pick", "revert", "tag",
];

/// Whether a command is git writing the repository: `git commit …`, and
/// a chain of them joined by `&&`, such as `git add . && git commit -m x`.
/// Anything else in the chain, a pipe or a redirection makes it not one.
pub fn is_git_write(command: &str) -> bool {
    if command.contains(['|', ';', '>', '<', '`', '$']) {
        return false;
    }
    let parts: Vec<&str> = command.split("&&").map(str::trim).collect();
    !parts.is_empty()
        && parts.iter().all(|part| {
            let mut words = part.split_whitespace();
            let (Some("git"), Some(mut sub)) = (words.next(), words.next()) else {
                return false;
            };
            // `git -C dir commit`: past the options git takes first.
            let mut rest = words;
            while sub.starts_with('-') {
                if sub == "-C" || sub == "-c" {
                    let _ = rest.next();
                }
                match rest.next() {
                    Some(next) => sub = next,
                    None => return false,
                }
            }
            GIT_WRITES.contains(&sub)
        })
}

/// What "Always" keeps for a request: its key and the words for it.
pub fn always(request: &ApprovalRequest) -> (String, String) {
    match &request.kind {
        ApprovalKind::Host => (format!("network:{}", request.subject), request.subject.clone()),
        ApprovalKind::Command if is_git_write(&request.subject) => {
            ("commit".to_string(), "commit".to_string())
        }
        ApprovalKind::Command => (format!("command:{}", request.subject), request.subject.clone()),
        ApprovalKind::Edit => (format!("tool:Edit:{}", request.subject), format!("edit {}", request.subject)),
        ApprovalKind::Tool { name } => {
            let what = request.suggestion.clone().unwrap_or_else(|| request.subject.clone());
            (format!("tool:{name}:{what}"), format!("{name} {what}"))
        }
    }
}

/// The Always button's words.
pub fn always_button(request: &ApprovalRequest) -> String {
    match &request.kind {
        ApprovalKind::Host => format!("Always allow {} here", request.subject),
        ApprovalKind::Command if is_git_write(&request.subject) => "Always let Work commit here".to_string(),
        ApprovalKind::Command => "Always allow this command here".to_string(),
        ApprovalKind::Edit | ApprovalKind::Tool { .. } => "Always allow this here".to_string(),
    }
}

/// The grant that answers a request, if the repository has one.
pub fn covering<'a>(grants: &'a [WorkGrantRecord], request: &ApprovalRequest) -> Option<&'a WorkGrantRecord> {
    let (key, _) = always(request);
    grants.iter().find(|grant| grant.key == key)
}

/// Whether the repository lets Work commit.
pub fn commits_granted(grants: &[WorkGrantRecord]) -> bool {
    grants.iter().any(|grant| grant.key == "commit")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: ApprovalKind, subject: &str) -> ApprovalRequest {
        ApprovalRequest {
            kind,
            subject: subject.into(),
            detail: None,
            suggestion: None,
        }
    }

    fn grant(key: &str) -> WorkGrantRecord {
        WorkGrantRecord {
            id: key.into(),
            repo: "a/b".into(),
            key: key.into(),
            label: key.into(),
            created_at: "2026-09-22T10:00:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn git_writes_are_recognised_and_nothing_else_is() {
        for yes in [
            "git commit -m \"Add a README\"",
            "git add . && git commit -m x",
            "git -C /wt commit -am x",
            "git checkout -b feature",
        ] {
            assert!(is_git_write(yes), "{yes}");
        }
        for no in [
            "git push origin main",
            "git status",
            "git commit -m x && curl example.com",
            "git commit -m $(cat msg)",
            "git log | head",
            "echo git commit",
            "git",
        ] {
            assert!(!is_git_write(no), "{no}");
        }
    }

    #[test]
    fn always_keys_on_what_the_request_is_about() {
        let host = request(ApprovalKind::Host, "example.com");
        assert_eq!(always(&host).0, "network:example.com");
        assert_eq!(always_button(&host), "Always allow example.com here");
        assert_eq!(title(&host), "Connect to example.com");

        let commit = request(ApprovalKind::Command, "git commit -m x");
        assert_eq!(always(&commit).0, "commit");
        assert_eq!(always_button(&commit), "Always let Work commit here");

        let command = request(ApprovalKind::Command, "cargo publish --dry-run");
        assert_eq!(always(&command).0, "command:cargo publish --dry-run");
        assert_eq!(one_line(&command), "Run outside the sandbox: cargo publish --dry-run");

        let tool = ApprovalRequest {
            suggestion: Some("domain:docs.rs".into()),
            ..request(ApprovalKind::Tool { name: "WebFetch".into() }, "Fetch docs.rs")
        };
        assert_eq!(always(&tool).0, "tool:WebFetch:domain:docs.rs");
    }

    #[test]
    fn a_grant_covers_exactly_what_it_was_given_for() {
        let grants = [grant("network:example.com"), grant("commit"), grant("command:make test")];
        assert!(covering(&grants, &request(ApprovalKind::Host, "example.com")).is_some());
        assert!(covering(&grants, &request(ApprovalKind::Host, "evil.example.com")).is_none());
        assert!(covering(&grants, &request(ApprovalKind::Command, "git add . && git commit -m y")).is_some());
        assert!(covering(&grants, &request(ApprovalKind::Command, "make test")).is_some());
        assert!(covering(&grants, &request(ApprovalKind::Command, "make test ")).is_none());
        assert!(covering(&grants, &request(ApprovalKind::Command, "git push")).is_none());
        assert!(commits_granted(&grants));
        assert!(!commits_granted(&grants[..1]));
    }
}
