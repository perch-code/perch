//! Whether the Changescape skill is installed for Claude Code: a
//! `SKILL.md` whose front matter says `name: changescape`, in a skill
//! folder of its own or in a plugin's.

use std::path::{Path, PathBuf};

/// How deep under `~/.claude/plugins` a plugin's `skills` folder can be.
const PLUGIN_DEPTH: usize = 4;

/// The installed skill's folder, under the user's home.
pub fn skill_available() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    skill_available_in(&home)
}

/// The installed skill's folder under `home`: `.claude/skills/*/`, or
/// `.claude/plugins/**/skills/*/` with `**` at most four folders deep.
fn skill_available_in(home: &Path) -> Option<PathBuf> {
    let claude = home.join(".claude");
    find_in_skills(&claude.join("skills")).or_else(|| find_in_plugins(&claude.join("plugins"), 0))
}

/// Whether the skill in `dir` writes a tour: its page schema has one.
/// A skill from before the tour still reports page format 3, so its
/// schema is the only sign.
pub fn skill_writes_tour(dir: &Path) -> bool {
    let Ok(bytes) = std::fs::read(dir.join("schema/page.v3.schema.json")) else {
        return false;
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .is_ok_and(|schema| schema.pointer("/properties/tour").is_some())
}

fn find_in_skills(skills: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(skills)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .find(|dir| is_changescape(&dir.join("SKILL.md")))
}

fn find_in_plugins(dir: &Path, depth: usize) -> Option<PathBuf> {
    if let Some(found) = find_in_skills(&dir.join("skills")) {
        return Some(found);
    }
    if depth >= PLUGIN_DEPTH {
        return None;
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name != "skills"))
        .collect();
    dirs.sort();
    dirs.into_iter()
        .find_map(|child| find_in_plugins(&child, depth + 1))
}

fn is_changescape(skill_md: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(skill_md) else {
        return false;
    };
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return false;
    }
    lines
        .take_while(|line| line.trim() != "---")
        .filter_map(|line| line.strip_prefix("name:"))
        .any(|name| name.trim().trim_matches(['"', '\'']) == "changescape")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: x\n---\n# Body\nname: changescape\n"),
        )
        .unwrap();
    }

    #[test]
    fn finds_a_user_skill() {
        let home = tempfile::tempdir().unwrap();
        write_skill(&home.path().join(".claude/skills/other"), "other");
        write_skill(&home.path().join(".claude/skills/cs"), "changescape");
        assert_eq!(
            skill_available_in(home.path()),
            Some(home.path().join(".claude/skills/cs"))
        );
    }

    #[test]
    fn finds_a_plugin_skill_within_four_folders() {
        let home = tempfile::tempdir().unwrap();
        let near = home
            .path()
            .join(".claude/plugins/cache/market/changescape/2.0.0/skills/changescape");
        write_skill(&near, "\"changescape\"");
        assert_eq!(skill_available_in(home.path()), Some(near));
    }

    #[test]
    fn ignores_a_plugin_skill_too_deep() {
        let home = tempfile::tempdir().unwrap();
        write_skill(
            &home
                .path()
                .join(".claude/plugins/a/b/c/d/e/skills/changescape"),
            "changescape",
        );
        assert_eq!(skill_available_in(home.path()), None);
    }

    #[test]
    fn a_skill_writes_a_tour_when_its_schema_has_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!skill_writes_tour(dir.path()));
        std::fs::create_dir_all(dir.path().join("schema")).unwrap();
        let schema = dir.path().join("schema/page.v3.schema.json");
        std::fs::write(&schema, r#"{"properties": {"sections": {}}}"#).unwrap();
        assert!(!skill_writes_tour(dir.path()));
        std::fs::write(&schema, r#"{"properties": {"tour": {}}}"#).unwrap();
        assert!(skill_writes_tour(dir.path()));
    }

    #[test]
    fn reads_the_name_from_front_matter_only() {
        let home = tempfile::tempdir().unwrap();
        // The body says `name: changescape`, the front matter does not.
        write_skill(&home.path().join(".claude/skills/other"), "other");
        assert_eq!(skill_available_in(home.path()), None);
    }
}
