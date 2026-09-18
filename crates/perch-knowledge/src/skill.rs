//! The skill file that tells an agent where the files are.
//!
//! The description decides whether the skill loads, so it names every
//! project and the phrases a person uses when they want history. The body
//! is prescriptive: every `ls` an agent has to do is a chance to wander.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct SkillConfig {
    /// Every project the store holds, for the description.
    pub projects: Vec<String>,
    /// The directories to write `SKILL.md` into.
    pub dirs: Vec<PathBuf>,
}

pub fn render(root: &Path, projects: &[String]) -> String {
    let root = root.display();
    let names = if projects.is_empty() { "your projects".to_string() } else { projects.join(", ") };
    let example = projects.first().map(String::as_str).unwrap_or("<project>");

    format!(
        "---
name: perch
description: Discussion history for {names} from GitHub pull requests and issues. Use when asked why something was decided, what was discussed, or for the context behind a change, a pull request or an issue.
---

Perch archives discussion about these projects as markdown under
`{root}`. Start with `{root}/index.md`, which maps
each project to its git repositories; match `git remote get-url origin`
against it to find the project for the repository you are in.

Layout, per project:

    <project>/index.md                   what is here and how current it is
    <project>/github/<owner>/<repo>/pulls/<n>.md
                                         description, comments, reviews
    <project>/github/<owner>/<repo>/issues/<n>.md
                                         issue, comments
    <project>/raw/                       machine records; read the markdown instead

Search with `rg` before reading anything whole:

    rg -n \"manifest fence\" {root}/{example}
    rg -n --glob 'pulls/8*.md' epoch {root}/{example}
    rg -n --glob 'issues/*.md' epoch {root}/{example}

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
    fn the_description_names_every_project_and_no_other() {
        let text = render(Path::new("/k"), &["slatedb".into(), "perch".into()]);
        let d = description(&text);
        assert!(d.contains("slatedb, perch"), "{d}");
        assert!(!d.contains("<project>"));
        assert!(!text.contains("your projects"));
    }

    #[test]
    fn the_root_is_the_one_configured() {
        let text = render(Path::new("/somewhere/else"), &["perch".into()]);
        assert!(text.contains("`/somewhere/else/index.md`"));
        assert!(text.contains("rg -n \"manifest fence\" /somewhere/else/perch"));
    }

    #[test]
    fn issues_are_named_beside_pull_requests() {
        let text = render(Path::new("/k"), &["perch".into()]);
        assert!(description(&text).contains("from GitHub pull requests and issues"));
        assert!(text.contains("<project>/github/<owner>/<repo>/issues/<n>.md"));
        assert!(text.contains("<project>/github/<owner>/<repo>/pulls/<n>.md"));
    }

    #[test]
    fn frontmatter_is_well_formed() {
        let text = render(Path::new("/k"), &[]);
        assert!(text.starts_with("---\nname: perch\ndescription: "));
        assert_eq!(text.matches("\n---\n").count(), 1);
    }
}
