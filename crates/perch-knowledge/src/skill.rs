//! The skill file that tells an agent where the files are.
//!
//! The description decides whether the skill loads, so it names every
//! repository and the phrases a person uses when they want history. The
//! body is prescriptive: every `ls` an agent has to do is a chance to
//! wander.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct SkillConfig {
    /// Every repository the store holds, as `owner/repo`, for the
    /// description.
    pub repos: Vec<String>,
    /// The directories to write `SKILL.md` into.
    pub dirs: Vec<PathBuf>,
}

pub fn render(root: &Path, repos: &[String]) -> String {
    let root = root.display();
    let names = if repos.is_empty() { "your repositories".to_string() } else { repos.join(", ") };
    let example = repos.first().map(String::as_str).unwrap_or("<owner>/<repo>");

    format!(
        "---
name: perch
description: Discussion history for {names} from GitHub pull requests and issues, and the questions the reader asked an agent about them in Perch. Use when asked why something was decided, what was discussed, what was asked before, or for the context behind a change, a pull request or an issue.
---

Perch archives discussion about these repositories as markdown under
`{root}`. Start with `{root}/index.md`, which maps
each repository to its directory; match `git remote get-url origin`
against `owner/repo` to find the directory for the repository you are in.

A repository declared in Perch is archived whole: its open pull requests
and issues as they move, and its closed history oldest first, so the
closed history is complete once the backfill has run. A closed pull
request or issue with no file here is one the backfill has not reached;
`perch --backfill {example}` runs it in the foreground and says whether
it is caught up.

Layout, per repository:

    <owner>/<repo>/index.md              what is here and how current it is
    <owner>/<repo>/github/pulls/<n>.md   description, comments, reviews
    <owner>/<repo>/github/issues/<n>.md  issue, comments
    <owner>/<repo>/perch/pulls/<n>.md
    <owner>/<repo>/perch/issues/<n>.md
    <owner>/<repo>/perch/branches/<branch>.md
                                         the reader's own threads with an agent
    <owner>/<repo>/raw/                  machine records; read the markdown instead

Two kinds of file, and they are not the same thing. Under `github/` is
what was said on GitHub, in public, by the people named. Under `perch/`
are questions the reader asked an agent on their own machine, with the
answers; the file's front matter says `source: perch`. Nobody on GitHub
has seen those, and they are not anyone's position but the reader's own
curiosity. Quote the first as what was decided; treat the second as
what the reader already looked into, and say which is which.

Search with `rg` before reading anything whole:

    rg -n \"manifest fence\" {root}/{example}
    rg -n --glob 'pulls/8*.md' epoch {root}/{example}
    rg -n --glob 'issues/*.md' epoch {root}/{example}
    rg -n --glob 'perch/**' epoch {root}/{example}

Every message carries its author, time and a permalink. Cite the permalink
when you rely on one. The files are updated while Perch runs, so read again
rather than remembering.
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn description(text: &str) -> String {
        text.lines()
            .find(|l| l.starts_with("description:"))
            .expect("a description line")
            .to_string()
    }

    #[test]
    fn the_description_names_every_repository_and_no_other() {
        let text = render(Path::new("/k"), &["slatedb/slatedb".into(), "criccomini/perch".into()]);
        let d = description(&text);
        assert!(d.contains("slatedb/slatedb, criccomini/perch"), "{d}");
        assert!(!d.contains("<owner>/<repo>"));
        assert!(!text.contains("your repositories"));
    }

    #[test]
    fn the_root_is_the_one_configured() {
        let text = render(Path::new("/somewhere/else"), &["criccomini/perch".into()]);
        assert!(text.contains("`/somewhere/else/index.md`"));
        assert!(text.contains("rg -n \"manifest fence\" /somewhere/else/criccomini/perch"));
    }

    #[test]
    fn issues_are_named_beside_pull_requests() {
        let text = render(Path::new("/k"), &["criccomini/perch".into()]);
        assert!(description(&text).contains("from GitHub pull requests and issues"));
        assert!(text.contains("<owner>/<repo>/github/issues/<n>.md"));
        assert!(text.contains("<owner>/<repo>/github/pulls/<n>.md"));
    }

    #[test]
    fn the_remote_is_matched_against_the_repository_name() {
        let text = render(Path::new("/k"), &[]);
        assert!(text.contains("match `git remote get-url origin`\nagainst `owner/repo`"), "{text}");
        assert!(text.contains("<owner>/<repo>/perch/branches/<branch>.md"));
    }

    #[test]
    fn closed_history_is_complete_once_the_backfill_has_run_and_the_command_is_named() {
        let text = render(Path::new("/k"), &["slatedb/slatedb".into()]);
        assert!(text.contains("closed history is complete once the backfill has run"), "{text}");
        assert!(text.contains("`perch --backfill slatedb/slatedb`"), "{text}");
    }

    #[test]
    fn frontmatter_is_well_formed() {
        let text = render(Path::new("/k"), &[]);
        assert!(text.starts_with("---\nname: perch\ndescription: "));
        assert_eq!(text.matches("\n---\n").count(), 1);
    }
}
