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

/// The request in one line, for a step and a run's activity: the heading,
/// and the command when that is what it is about.
pub fn one_line(request: &ApprovalRequest) -> String {
    match &request.kind {
        ApprovalKind::Command => format!("Run outside the sandbox: {}", request.subject),
        ApprovalKind::Tool { .. } => format!("{}: {}", title(request), request.subject),
        _ => title(request),
    }
}

/// Git subcommands that record the worktree in the repository, which the
/// commit grant covers. Nothing here writes the worktree: `checkout`,
/// `restore`, `reset`, `stash`, `merge`, `rebase` and the like would
/// write it outside the sandbox from objects and state the sandboxed run
/// could make, the paths the sandbox protects (`.claude`, `.mcp.json`)
/// among them, and `rebase --continue` runs the `exec` lines of a todo
/// it could write. Those ask the reader.
const GIT_WRITES: &[&str] = &["add", "commit", "tag"];

/// Long options that run a program or read a file into the commit. Git
/// takes any unambiguous prefix of a long option, so a prefix of one of
/// these is refused too.
const GIT_REFUSED_LONG: &[&str] = &["config", "gpg-sign", "local-user", "sign", "template"];

/// The short options refused after `git <sub>`: `-c` everywhere, those
/// that sign (running `gpg.program`), and `commit -t`, a template.
fn git_refused_short(sub: &str) -> &'static str {
    match sub {
        "commit" => "cSt",
        "tag" => "csu",
        _ => "c",
    }
}

/// Whether a command is git writing the repository: `git commit …`, and
/// a chain of them joined by `&&`, such as `git add . && git commit -m x`.
/// The grant answers such a request without the reader seeing it, so
/// anything that could do more than write this worktree's repository
/// makes it not one, and the reader is asked for it as for any command:
/// anything else in the chain, anything the shell would expand, glob or
/// redirect, any option before the subcommand (`-c core.hooksPath=…`,
/// `-C elsewhere`), and the options [`git_refused_short`] and
/// [`GIT_REFUSED_LONG`] name.
pub fn is_git_write(command: &str) -> bool {
    let Some(commands) = shell_words(command) else {
        return false;
    };
    commands.iter().all(|words| match words.as_slice() {
        [git, sub, args @ ..] if git == "git" && GIT_WRITES.contains(&sub.as_str()) => {
            args.iter().all(|arg| !git_refused(sub, arg))
        }
        _ => false,
    })
}

/// Whether `arg` is an option [`is_git_write`] refuses after `git <sub>`.
/// A short cluster is refused if any letter in it is; that also refuses
/// an attached value such as `-mSecond`, which only means the reader is
/// asked.
fn git_refused(sub: &str, arg: &str) -> bool {
    if let Some(long) = arg.strip_prefix("--") {
        let name = long.split('=').next().unwrap_or_default();
        !name.is_empty()
            && GIT_REFUSED_LONG
                .iter()
                .any(|refused| refused.starts_with(name))
    } else if let Some(short) = arg.strip_prefix('-') {
        short.chars().any(|c| git_refused_short(sub).contains(c))
    } else {
        false
    }
}

/// A command split as the shell would split it: into the commands joined
/// by `&&`, each into its unquoted words. None when the shell would do
/// more than split and unquote: expand a variable, a command or a glob,
/// redirect, pipe, run something in the background or on another line.
fn shell_words(command: &str) -> Option<Vec<Vec<String>>> {
    let mut commands = vec![Vec::new()];
    let mut word: Option<String> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => commands.last_mut()?.extend(word.take()),
            '&' if chars.next_if_eq(&'&').is_some() => {
                commands.last_mut()?.extend(word.take());
                commands.push(Vec::new());
            }
            '\'' => {
                let word = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                let word = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '$' | '`' | '\\' => return None,
                        c => word.push(c),
                    }
                }
            }
            c if c.is_ascii_alphanumeric() || "-_./:=@+,".contains(c) => {
                word.get_or_insert_with(String::new).push(c)
            }
            _ => return None,
        }
    }
    commands.last_mut()?.extend(word);
    Some(commands)
}

/// What "Always" keeps for a request: its key and the words for it.
pub fn always(request: &ApprovalRequest) -> (String, String) {
    match &request.kind {
        ApprovalKind::Host => (
            format!("network:{}", request.subject),
            request.subject.clone(),
        ),
        ApprovalKind::Command if is_git_write(&request.subject) => {
            ("commit".to_string(), "commit".to_string())
        }
        ApprovalKind::Command => (
            format!("command:{}", request.subject),
            request.subject.clone(),
        ),
        ApprovalKind::Edit => (
            format!("tool:Edit:{}", request.subject),
            format!("edit {}", request.subject),
        ),
        ApprovalKind::Tool { name } => {
            let what = request
                .suggestion
                .clone()
                .unwrap_or_else(|| request.subject.clone());
            // A grant for the whole tool comes back named after the
            // tool, which would read "AskUserQuestion AskUserQuestion".
            let label = if what == *name {
                name.clone()
            } else {
                format!("{name} {what}")
            };
            (format!("tool:{name}:{what}"), label)
        }
    }
}

/// The Always button's words.
pub fn always_button(request: &ApprovalRequest) -> String {
    match &request.kind {
        ApprovalKind::Host => format!("Always allow {} here", request.subject),
        ApprovalKind::Command if is_git_write(&request.subject) => {
            "Always let Work commit here".to_string()
        }
        ApprovalKind::Command => "Always allow this command here".to_string(),
        ApprovalKind::Edit | ApprovalKind::Tool { .. } => "Always allow this here".to_string(),
    }
}

/// The grant that answers a request, if the repository has one.
pub fn covering<'a>(
    grants: &'a [WorkGrantRecord],
    request: &ApprovalRequest,
) -> Option<&'a WorkGrantRecord> {
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
            "git add -u && git commit -s -m 'Fix the -c flag; && it'",
            "git commit --no-gpg-sign -m x",
            "git tag v1",
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
            "",
            "git commit -m x &&",
            "git commit -m x & curl example.com",
            "git commit -m x\ncurl example.com",
            "git commit -m \"$(cat msg)\"",
            "git commit -m 'unterminated",
            "git add *",
        ] {
            assert!(!is_git_write(no), "{no}");
        }
    }

    #[test]
    fn git_writes_that_could_leave_the_repository_are_not_git_writes() {
        for no in [
            // A hook the agent wrote in its worktree, run outside the sandbox.
            "git -c core.hooksPath=./h commit -m x",
            "git add . && git -c core.fsmonitor=./f commit -m x",
            "git -c core.sshCommand=./s commit -m x",
            // Another repository.
            "git -C /some/other/repo commit -am x",
            "git -C . commit -m x",
            "git --git-dir=/other/.git commit -m x",
            "git --work-tree=/other add .",
            // Signing runs gpg.program.
            "git -c gpg.program=./g commit -S -m x",
            "git commit -S -m x",
            "git commit -aS -m x",
            "git commit -Skey -m x",
            "git commit --gpg-sign -m x",
            "git commit --gpg-sign=key -m x",
            "git commit --gpg -m x",
            "git tag -s v1 -m x",
            "git tag -u key v1 -m x",
            "git tag --sign v1 -m x",
            // Config and programs by subcommand option.
            "git commit -c HEAD",
            "git rebase --exec ./x main",
            "git rebase --exe=./x main",
            "git rebase -x ./x main",
            "git merge -s ../../tmp/x main",
            "git merge --strategy=x main",
            "git commit --template=./t",
            "git commit -t ./t",
            "git checkout --recurse-submodules main",
            // What writes the worktree, from objects or a todo the run
            // could write.
            "git checkout -- .claude/settings.json",
            "git checkout -b feature",
            "git restore --source=HEAD~1 .mcp.json",
            "git reset --hard HEAD~1",
            "git stash pop",
            "git mv notes.md .mcp.json",
            "git rm -r .claude",
            "git switch main",
            "git merge feature",
            "git rebase main",
            "git rebase --continue",
            "git cherry-pick --continue",
            "git revert --continue",
            // Quoting does not hide an option from git.
            "git commit '--gpg-sign' -m x",
            "git \"-c\" core.hooksPath=./h commit -m x",
            "git commit --gpg\"-sign\" -m x",
        ] {
            assert!(!is_git_write(no), "{no}");
            let request = request(ApprovalKind::Command, no);
            assert_eq!(always(&request).0, format!("command:{no}"));
            assert!(covering(&[grant("commit")], &request).is_none(), "{no}");
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
        assert_eq!(
            one_line(&command),
            "Run outside the sandbox: cargo publish --dry-run"
        );

        let tool = ApprovalRequest {
            suggestion: Some("domain:docs.rs".into()),
            ..request(
                ApprovalKind::Tool {
                    name: "WebFetch".into(),
                },
                "Fetch docs.rs",
            )
        };
        assert_eq!(
            always(&tool),
            (
                "tool:WebFetch:domain:docs.rs".to_string(),
                "WebFetch domain:docs.rs".to_string()
            )
        );

        let whole_tool = ApprovalRequest {
            suggestion: Some("AskUserQuestion".into()),
            ..request(
                ApprovalKind::Tool {
                    name: "AskUserQuestion".into(),
                },
                "AskUserQuestion",
            )
        };
        assert_eq!(
            always(&whole_tool),
            (
                "tool:AskUserQuestion:AskUserQuestion".to_string(),
                "AskUserQuestion".to_string()
            )
        );
    }

    #[test]
    fn a_grant_covers_exactly_what_it_was_given_for() {
        let grants = [
            grant("network:example.com"),
            grant("commit"),
            grant("command:make test"),
        ];
        assert!(covering(&grants, &request(ApprovalKind::Host, "example.com")).is_some());
        assert!(covering(&grants, &request(ApprovalKind::Host, "evil.example.com")).is_none());
        assert!(covering(
            &grants,
            &request(ApprovalKind::Command, "git add . && git commit -m y")
        )
        .is_some());
        assert!(covering(&grants, &request(ApprovalKind::Command, "make test")).is_some());
        assert!(covering(&grants, &request(ApprovalKind::Command, "make test ")).is_none());
        assert!(covering(&grants, &request(ApprovalKind::Command, "git push")).is_none());
        assert!(commits_granted(&grants));
        assert!(!commits_granted(&grants[..1]));
    }
}
