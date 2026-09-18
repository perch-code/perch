//! Ingest against a temporary root.

use jiff::Timestamp;
use perch_knowledge::{Knowledge, Message, SkillConfig, Source};

fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn msg(id: &str, at_: &str, body: &str) -> Message {
    Message {
        source: Source::GitHub,
        project: "slatedb".into(),
        container: "slatedb/slatedb#812".into(),
        thread: None,
        id: id.into(),
        reply_to: None,
        author: "chris".into(),
        at: at(at_),
        edited_at: None,
        body: body.into(),
        url: format!("https://github.com/slatedb/slatedb/pull/812#{id}"),
        location: None,
    }
}

fn discussion() -> Vec<Message> {
    let mut opener = msg("pr-812", "2026-09-14T14:02:00Z", "# Fence the manifest on compaction\n\nThe description.");
    opener.author = "rohan".into();
    let q = msg("comment-1", "2026-09-15T09:14:00Z", "Should this fence every write or only compactions?");
    let mut a = msg("comment-2", "2026-09-15T09:20:00Z", "Only compactions. Writes go through the WAL fence already.");
    a.author = "rohan".into();
    a.reply_to = Some("comment-1".into());
    let mut review = msg("review-9", "2026-09-16T11:40:00Z", "approved");
    review.thread = Some("review-9".into());
    let mut line = msg("review-comment-3", "2026-09-16T11:38:00Z", "This assumes the epoch is monotonic. Worth a comment.");
    line.thread = Some("review-9".into());
    line.reply_to = Some("review-9".into());
    line.location = Some("src/manifest.rs:41".into());
    vec![opener, q, a, review, line]
}

const RENDERED: &str = "# slatedb/slatedb#812: Fence the manifest on compaction

**rohan** opened 2026-09-14 14:02 [↗](https://github.com/slatedb/slatedb/pull/812#pr-812)

The description.

---

**chris** 2026-09-15 09:14 [↗](https://github.com/slatedb/slatedb/pull/812#comment-1)

Should this fence every write or only compactions?

**rohan** 2026-09-15 09:20 [↗](https://github.com/slatedb/slatedb/pull/812#comment-2) replying to chris 09:14

Only compactions. Writes go through the WAL fence already.

---

## Review: rohan approved 2026-09-16 11:40 [↗](https://github.com/slatedb/slatedb/pull/812#review-9)

**chris** 2026-09-16 11:38 [↗](https://github.com/slatedb/slatedb/pull/812#review-comment-3) on `src/manifest.rs:41` replying to rohan 11:40

This assumes the epoch is monotonic. Worth a comment.
";

#[test]
fn the_rendered_file_matches_the_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    let mut d = discussion();
    d[3].author = "rohan".into();
    let n = k.ingest(&d).unwrap();
    assert_eq!(n.added, 5);
    assert_eq!(n.changed, 0);

    let file = tmp.path().join("slatedb/github/slatedb/slatedb/pulls/812.md");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), RENDERED);
    assert!(tmp.path().join("slatedb/raw/github/slatedb-slatedb.jsonl").is_file());
    assert!(tmp.path().join("slatedb/index.md").is_file());
    let root = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
    assert!(root.contains("| slatedb | slatedb/slatedb | 5 |"), "{root}");
}

#[test]
fn ingesting_again_is_a_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    k.ingest(&discussion()).unwrap();
    let raw = tmp.path().join("slatedb/raw/github/slatedb-slatedb.jsonl");
    let md = tmp.path().join("slatedb/github/slatedb/slatedb/pulls/812.md");
    let (raw_before, md_before) =
        (std::fs::read_to_string(&raw).unwrap(), std::fs::read_to_string(&md).unwrap());

    let n = k.ingest(&discussion()).unwrap();
    assert_eq!(n.total(), 0);
    assert_eq!(std::fs::read_to_string(&raw).unwrap(), raw_before);
    assert_eq!(std::fs::read_to_string(&md).unwrap(), md_before);
}

#[test]
fn an_edited_body_replaces_the_old_one() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    k.ingest(&discussion()).unwrap();

    let mut edited = discussion();
    edited[1].body = "Should this fence only compactions?".into();
    edited[1].edited_at = Some(at("2026-09-15T10:00:00Z"));
    let n = k.ingest(&edited).unwrap();
    assert_eq!(n, perch_knowledge::Ingested { added: 0, changed: 1 });

    let raw = std::fs::read_to_string(tmp.path().join("slatedb/raw/github/slatedb-slatedb.jsonl")).unwrap();
    assert_eq!(raw.lines().count(), 5, "a replaced record does not add a line");
    let md = std::fs::read_to_string(tmp.path().join("slatedb/github/slatedb/slatedb/pulls/812.md")).unwrap();
    assert!(md.contains("Should this fence only compactions?"));
    assert!(!md.contains("every write"));
    assert!(md.contains("(edited)"));
}

#[test]
fn an_unclosed_fence_is_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    let mut d = discussion();
    d[1].body = "Look:\n```rust\nlet x = 1;".into();
    k.ingest(&d).unwrap();
    let md = std::fs::read_to_string(tmp.path().join("slatedb/github/slatedb/slatedb/pulls/812.md")).unwrap();
    let fences = md.lines().filter(|l| l.trim_start().starts_with("```")).count();
    assert_eq!(fences % 2, 0, "{md}");
    // The message after it is still its own paragraph, not code.
    assert!(md.contains("```\n\n**rohan** 2026-09-15 09:20"));
}

#[test]
fn rerender_rebuilds_everything_outside_raw() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    k.ingest(&discussion()).unwrap();
    let md = tmp.path().join("slatedb/github/slatedb/slatedb/pulls/812.md");
    let before = std::fs::read_to_string(&md).unwrap();
    let index_before = std::fs::read_to_string(tmp.path().join("slatedb/index.md")).unwrap();

    std::fs::remove_dir_all(tmp.path().join("slatedb/github")).unwrap();
    std::fs::remove_file(tmp.path().join("slatedb/index.md")).unwrap();
    std::fs::remove_file(tmp.path().join("index.md")).unwrap();

    k.rerender().unwrap();
    assert_eq!(std::fs::read_to_string(&md).unwrap(), before);
    assert_eq!(std::fs::read_to_string(tmp.path().join("slatedb/index.md")).unwrap(), index_before);
    assert!(tmp.path().join("index.md").is_file());
}

#[test]
fn no_temporary_files_are_left_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    k.ingest(&discussion()).unwrap();
    let mut stack = vec![tmp.path().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                assert!(!p.to_string_lossy().ends_with(".tmp"), "{p:?}");
            }
        }
    }
}

/// Issue 42 in the same repository as the pull request: an opener and one
/// comment.
fn issue() -> Vec<Message> {
    let mut opener = msg("issue-42", "2026-09-13T08:00:00Z", "# Manifest races with compaction\n\nSeen twice.");
    opener.container = "slatedb/slatedb#issue-42".into();
    opener.url = "https://github.com/slatedb/slatedb/issues/42".into();
    let mut reply = msg("comment-7", "2026-09-13T09:30:00Z", "Repro attached.");
    reply.container = "slatedb/slatedb#issue-42".into();
    reply.url = "https://github.com/slatedb/slatedb/issues/42#issuecomment-7".into();
    vec![opener, reply]
}

#[test]
fn an_issue_renders_beside_the_repository_pulls() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    k.ingest(&discussion()).unwrap();
    let n = k.ingest(&issue()).unwrap();
    assert_eq!(n.added, 2);

    let md = std::fs::read_to_string(tmp.path().join("slatedb/github/slatedb/slatedb/issues/42.md")).unwrap();
    assert!(md.starts_with("# slatedb/slatedb#42: Manifest races with compaction\n"), "{md}");
    assert!(md.contains("**chris** opened 2026-09-13 08:00 [↗](https://github.com/slatedb/slatedb/issues/42)"));
    assert!(md.contains("Repro attached."));
    assert!(!md.contains("issue-42"), "the heading names the issue as GitHub does");

    // One raw file per repository, with both in it.
    let raw = std::fs::read_to_string(tmp.path().join("slatedb/raw/github/slatedb-slatedb.jsonl")).unwrap();
    assert_eq!(raw.lines().count(), 7);
    assert!(tmp.path().join("slatedb/github/slatedb/slatedb/pulls/812.md").is_file());

    // The project index knows one repository and lists both files under it.
    let index = std::fs::read_to_string(tmp.path().join("slatedb/index.md")).unwrap();
    assert!(index.contains("Repositories: slatedb/slatedb\n"), "{index}");
    assert!(index.contains("| slatedb/slatedb#812 | github/slatedb/slatedb/pulls/812.md | 5 |"), "{index}");
    assert!(index.contains("| slatedb/slatedb#42 | github/slatedb/slatedb/issues/42.md | 2 |"), "{index}");
    let root = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
    assert!(root.contains("| slatedb | slatedb/slatedb | 7 |"), "{root}");
}

#[test]
fn the_skill_lands_in_every_directory_named() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path().join("knowledge"));
    let dirs = vec![tmp.path().join("claude/skills/perch"), tmp.path().join("agents/skills/perch")];
    k.write_skill(&SkillConfig { projects: vec!["slatedb".into()], dirs: dirs.clone() }).unwrap();
    for dir in dirs {
        let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(text.contains("slatedb"));
        assert!(text.contains(&tmp.path().join("knowledge").display().to_string()));
        assert!(text.contains("issues/<n>.md"));
    }
}
