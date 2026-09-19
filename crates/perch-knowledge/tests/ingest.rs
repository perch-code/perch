//! Ingest against a temporary root.

use jiff::Timestamp;
use perch_knowledge::{Error, Knowledge, Message, SkillConfig, Source};

fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn msg(id: &str, at_: &str, body: &str) -> Message {
    Message {
        source: Source::GitHub,
        repo: "slatedb/slatedb".into(),
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

    let file = tmp.path().join("slatedb/slatedb/github/pulls/812.md");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), RENDERED);
    assert!(tmp.path().join("slatedb/slatedb/raw/github.jsonl").is_file());
    assert!(tmp.path().join("slatedb/slatedb/index.md").is_file());
    let root = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
    assert!(root.contains("| slatedb/slatedb | slatedb/slatedb/ | 5 |"), "{root}");
}

#[test]
fn ingesting_again_is_a_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    k.ingest(&discussion()).unwrap();
    let raw = tmp.path().join("slatedb/slatedb/raw/github.jsonl");
    let md = tmp.path().join("slatedb/slatedb/github/pulls/812.md");
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

    let raw = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/raw/github.jsonl")).unwrap();
    assert_eq!(raw.lines().count(), 5, "a replaced record does not add a line");
    let md = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/github/pulls/812.md")).unwrap();
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
    let md = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/github/pulls/812.md")).unwrap();
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
    let md = tmp.path().join("slatedb/slatedb/github/pulls/812.md");
    let before = std::fs::read_to_string(&md).unwrap();
    let index_before = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/index.md")).unwrap();
    let root_before = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();

    std::fs::remove_dir_all(tmp.path().join("slatedb/slatedb/github")).unwrap();
    std::fs::remove_file(tmp.path().join("slatedb/slatedb/index.md")).unwrap();
    std::fs::remove_file(tmp.path().join("index.md")).unwrap();

    k.rerender().unwrap();
    assert_eq!(std::fs::read_to_string(&md).unwrap(), before);
    assert_eq!(std::fs::read_to_string(tmp.path().join("slatedb/slatedb/index.md")).unwrap(), index_before);
    assert_eq!(std::fs::read_to_string(tmp.path().join("index.md")).unwrap(), root_before);
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

    let md = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/github/issues/42.md")).unwrap();
    assert!(md.starts_with("# slatedb/slatedb#42: Manifest races with compaction\n"), "{md}");
    assert!(md.contains("**chris** opened 2026-09-13 08:00 [↗](https://github.com/slatedb/slatedb/issues/42)"));
    assert!(md.contains("Repro attached."));
    assert!(!md.contains("issue-42"), "the heading names the issue as GitHub does");

    // One raw file per repository and source, with both in it.
    let raw = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/raw/github.jsonl")).unwrap();
    assert_eq!(raw.lines().count(), 7);
    assert!(tmp.path().join("slatedb/slatedb/github/pulls/812.md").is_file());

    // The repository index is headed by the repository and lists both
    // files under it.
    let index = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/index.md")).unwrap();
    assert!(index.starts_with("# slatedb/slatedb\n"), "{index}");
    assert!(index.contains("| slatedb/slatedb#812 | github/pulls/812.md | 5 |"), "{index}");
    assert!(index.contains("| slatedb/slatedb#42 | github/issues/42.md | 2 |"), "{index}");
    let root = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
    assert!(root.contains("| slatedb/slatedb | slatedb/slatedb/ | 7 |"), "{root}");
}

/// One message on pull request 3 of a second repository.
fn other_repository() -> Message {
    let mut m = msg("pr-3", "2026-09-17T12:00:00Z", "# Group by owner\n\nThe description.");
    m.repo = "criccomini/perch".into();
    m.container = "criccomini/perch#3".into();
    m.url = "https://github.com/criccomini/perch/pull/3".into();
    m
}

#[test]
fn the_root_index_lists_every_repository_and_its_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    k.ingest(&discussion()).unwrap();
    k.ingest(&[other_repository()]).unwrap();

    assert!(tmp.path().join("criccomini/perch/github/pulls/3.md").is_file());
    assert!(tmp.path().join("criccomini/perch/raw/github.jsonl").is_file());
    assert_eq!(k.repos().unwrap(), ["criccomini/perch", "slatedb/slatedb"]);

    let root = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
    assert!(root.contains("| repository | directory | messages | updated |\n"), "{root}");
    assert!(root.contains("| criccomini/perch | criccomini/perch/ | 1 | 2026-09-17 12:00 |\n"), "{root}");
    assert!(root.contains("| slatedb/slatedb | slatedb/slatedb/ | 5 | 2026-09-16 11:40 |\n"), "{root}");
    // Each repository's own index knows only itself.
    let index = std::fs::read_to_string(tmp.path().join("criccomini/perch/index.md")).unwrap();
    assert!(index.contains("criccomini/perch#3"), "{index}");
    assert!(!index.contains("slatedb"), "{index}");
}

#[test]
fn an_old_layout_directory_beside_the_root_is_not_a_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    // The layout before this one filed a project at the top level, with
    // `raw/` one level down. Left alone, it must not be read, listed, or
    // rendered over.
    let old = tmp.path().join("slatedb");
    std::fs::create_dir_all(old.join("raw/github")).unwrap();
    std::fs::write(old.join("raw/github/slatedb-slatedb.jsonl"), "{\"project\":\"slatedb\"}\n").unwrap();
    std::fs::create_dir_all(old.join("github/slatedb/slatedb/pulls")).unwrap();
    std::fs::write(old.join("github/slatedb/slatedb/pulls/1.md"), "# old\n").unwrap();
    std::fs::write(old.join("index.md"), "# slatedb\n").unwrap();

    assert!(k.repos().unwrap().is_empty());
    k.ingest(&discussion()).unwrap();
    assert_eq!(k.repos().unwrap(), ["slatedb/slatedb"]);
    k.rerender().unwrap();

    let root = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
    assert!(root.contains("| slatedb/slatedb | slatedb/slatedb/ | 5 |"), "{root}");
    assert_eq!(root.lines().filter(|l| l.starts_with("| slatedb")).count(), 1, "{root}");
    assert_eq!(std::fs::read_to_string(old.join("index.md")).unwrap(), "# slatedb\n");
    assert_eq!(std::fs::read_to_string(old.join("github/slatedb/slatedb/pulls/1.md")).unwrap(), "# old\n");
}

#[test]
fn a_repository_that_is_not_owner_and_repo_is_refused_before_anything_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    for repo in ["../x/../y", "../../etc", "slatedb", ""] {
        let mut d = discussion();
        d[1].repo = repo.into();
        let err = k.ingest(&d).unwrap_err();
        assert!(matches!(err, Error::Repo(ref r) if r == repo), "{repo:?}: {err}");
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none(), "{repo:?} wrote something");
    }

    // Two segments that are each hostile are made safe and stay inside.
    let mut m = other_repository();
    m.repo = "../x".into();
    k.ingest(&[m]).unwrap();
    assert!(tmp.path().join("-/x/raw/github.jsonl").is_file());
    assert_eq!(k.repos().unwrap(), ["-/x"]);
    assert!(!tmp.path().parent().unwrap().join("x").exists());
}

#[test]
fn a_repository_named_with_a_leading_dot_comes_back_under_its_own_name() {
    // `acme/.github` is a real repository, and `acme/github` may be another.
    // Each gets its own directory, and the root index names both as they are.
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    let mut m = other_repository();
    m.repo = "acme/.github".into();
    m.container = "acme/.github#3".into();
    k.ingest(&[m]).unwrap();
    let mut m = other_repository();
    m.repo = "acme/github".into();
    m.container = "acme/github#3".into();
    m.body = "# The other one".into();
    k.ingest(&[m]).unwrap();

    assert!(tmp.path().join("acme/.github/raw/github.jsonl").is_file());
    assert!(tmp.path().join("acme/.github/github/pulls/3.md").is_file());
    assert!(tmp.path().join("acme/github/github/pulls/3.md").is_file());
    assert_eq!(k.repos().unwrap(), ["acme/.github", "acme/github"]);
    let root = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
    assert!(root.contains("| acme/.github | acme/.github/ | 1 |"), "{root}");
    assert!(root.contains("| acme/github | acme/github/ | 1 |"), "{root}");

    // A rerender finds the same repositories and rewrites the same files.
    k.rerender().unwrap();
    assert_eq!(k.repos().unwrap(), ["acme/.github", "acme/github"]);
    let page = std::fs::read_to_string(tmp.path().join("acme/.github/github/pulls/3.md")).unwrap();
    assert!(page.contains("Group by owner"), "{page}");
}

#[test]
fn the_skill_lands_in_every_directory_named() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path().join("knowledge"));
    let dirs = vec![tmp.path().join("claude/skills/perch"), tmp.path().join("agents/skills/perch")];
    k.write_skill(&SkillConfig { repos: vec!["slatedb/slatedb".into()], dirs: dirs.clone() }).unwrap();
    for dir in dirs {
        let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(text.contains("slatedb/slatedb"));
        assert!(text.contains(&tmp.path().join("knowledge").display().to_string()));
        assert!(text.contains("issues/<n>.md"));
    }
}

#[test]
fn the_reader_s_own_threads_land_beside_the_discussion_and_say_what_they_are() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    k.ingest(&discussion()).unwrap();

    let mut asked = msg("asked-q1", "2026-09-17T10:00:00Z", "Why is the fence here?");
    asked.source = Source::Perch;
    asked.thread = Some("t1".into());
    asked.author = "reader".into();
    asked.location = Some("src/manifest.rs:41".into());
    let mut answered = msg("answered-a1", "2026-09-17T10:01:00Z", "Because compaction can race the manifest.");
    answered.source = Source::Perch;
    answered.thread = Some("t1".into());
    answered.reply_to = Some("asked-q1".into());
    answered.author = "sonnet (claude-code, high)".into();
    k.ingest(&[asked, answered]).unwrap();

    // A file of its own, under perch/, in the same pull request's shape as
    // the GitHub file, with front matter that says what it is.
    let local = tmp.path().join("slatedb/slatedb/perch/pulls/812.md");
    let github = tmp.path().join("slatedb/slatedb/github/pulls/812.md");
    assert!(github.is_file());
    assert!(tmp.path().join("slatedb/slatedb/raw/perch.jsonl").is_file());
    let text = std::fs::read_to_string(&local).unwrap();
    assert!(text.starts_with("---\nsource: perch\nkind: local threads\n"), "{text}");
    assert!(text.contains("## Asked: reader Why is the fence here?"), "{text}");
    assert!(text.contains("**sonnet (claude-code, high)**"), "{text}");
    assert!(text.contains("on `src/manifest.rs:41`"), "{text}");
    // The GitHub file stays as it was: no front matter, nothing local in it.
    let public = std::fs::read_to_string(&github).unwrap();
    assert!(!public.starts_with("---"));
    assert!(!public.contains("fence here"));
    // The repository index lists both sources.
    let index = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/index.md")).unwrap();
    assert!(index.contains("## perch"), "{index}");
    assert!(index.contains("| slatedb/slatedb#812 | perch/pulls/812.md | 2 |"), "{index}");
}

#[test]
fn a_branch_with_nothing_on_github_files_its_threads_under_branches() {
    let tmp = tempfile::tempdir().unwrap();
    let k = Knowledge::open(tmp.path());
    let mut asked = msg("asked-q2", "2026-09-17T10:00:00Z", "Is the fence right?");
    asked.source = Source::Perch;
    asked.container = "slatedb/slatedb@chris/fence".into();
    asked.thread = Some("t2".into());
    asked.author = "reader".into();
    let mut answered = msg("answered-a2", "2026-09-17T10:01:00Z", "Yes.");
    answered.source = Source::Perch;
    answered.container = "slatedb/slatedb@chris/fence".into();
    answered.thread = Some("t2".into());
    answered.reply_to = Some("asked-q2".into());
    answered.author = "sonnet (claude-code, high)".into();
    k.ingest(&[asked, answered]).unwrap();

    let file = tmp.path().join("slatedb/slatedb/perch/branches/chris-fence.md");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("# slatedb/slatedb@chris/fence\n"), "{text}");
    let index = std::fs::read_to_string(tmp.path().join("slatedb/slatedb/index.md")).unwrap();
    assert!(index.contains("| slatedb/slatedb@chris/fence | perch/branches/chris-fence.md | 2 |"), "{index}");
}
