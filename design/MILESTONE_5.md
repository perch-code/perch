# Perch: Milestone 5, Knowledge

> Historical specification for a removed feature. Perch no longer builds or
> maintains a knowledge archive; see [README.md](../README.md).

## Goal

Give the coding agents on this machine the discussion behind the code.
Today Claude Code and Codex can read a repository and its git history, and
nothing else. Why a change was made, what was argued about, what was ruled
out, lives in pull request threads and in chat, and the agents never see it.

After this milestone there is a directory, `~/.perch/knowledge`, of plain
markdown that holds that discussion, and a skill installed for both agents
that says where it is and how it is laid out. An agent asked "why does the
manifest fence only compactions" runs `rg` over the directory and finds the
thread.

Pull request discussion from GitHub is the first source, because Perch
already talks to GitHub and already knows which pull requests matter. The
layout, the record, and the skill are made for more than one source. Discord
is the next milestone, and it is where the always-on process appears. This
milestone is a library and some files.

## Non-goals

- A database. Raw records are JSONL. SQLite comes back if something needs an
  indexed lookup, and nothing in this milestone does.
- Search. Ripgrep over the markdown is the search. No FTS, no embeddings,
  no query API, no `perch search` command.
- Discord, Slack, or any source that needs a connection Perch does not
  already hold.
- A daemon. Ingest runs inside Perch when a pull request is opened.
  Anything more continuous waits for a source that needs it.
- A UI. Nothing in the window shows the knowledge directory. The status line
  may say "3 comments archived" and that is all.
- Completeness. Only pull requests that are opened in Perch are archived.
  Backfilling a repository's whole history is a later milestone, after there
  is a process to run it in.
- Answering questions. The skill tells an agent where to look. It does not
  summarise, and Perch does not.

## Behaviour

Opening a pull request in Perch, from the inbox or from the Worktrees list,
fetches its discussion alongside the checkout: the description, the issue
comments, the reviews, and the review comments with the file and line they
sit on. Each becomes a record. The records go into the project's raw file,
and the pull request's markdown file is rendered from them.

Opening the same pull request again fetches again and adds only what is
new. A record is keyed by the source's own id, so re-ingest is a no-op and
an edited comment replaces its earlier body.

The skill is written the first time anything is ingested, and rewritten
whenever the set of projects changes. It names every project it holds, so
that an agent working in one of them recognises the skill as relevant.

Nothing here needs the network beyond what opening a pull request already
costs, and nothing blocks the review view. The discussion arrives after the
checkout, and a failure to fetch it is a status line, not an error.

## Projects

A project is the unit an agent works in, and it can span repositories. The
SlateDB Discord covers more than one repository, and a question about the
Go client is a question about SlateDB.

`config.toml` gains:

```toml
[[knowledge.project]]
name = "slatedb"
repos = ["slatedb/slatedb", "slatedb/slatedb-go"]
```

A repository not listed under any project gets a project named after the
repository, `perch` for `criccomini/perch`, so that nothing has to be
configured before the first file appears.

## Layout

```
~/.perch/knowledge/
  index.md                          # projects, their repos, sources, last updated
  slatedb/
    index.md                        # this project's repos and containers
    raw/
      github/slatedb-slatedb.jsonl  # one Message per line, append-only
    github/
      slatedb/slatedb/pulls/812.md
      slatedb/slatedb/pulls/815.md
```

`raw/` is the truth. Everything outside it is rendered from it and can be
deleted and rebuilt. Ingest appends to one raw file and re-renders the one
markdown file the records land in.

The root is `~/.perch/knowledge` and not the platform data directory Perch
uses for its database. Agents have to be told this path, it has to survive
a reinstall, and it has to be short enough to type. It is overridable with
`knowledge.root` in `config.toml`, and the skill is written with whatever
the root is.

The `raw/` file for a source is per container group rather than per
message: one file per repository for GitHub, one per channel for Discord
later. Small enough to rewrite when an edit lands, large enough that a
project is a handful of files.

## Model

In a new crate, `perch-knowledge`, with no dependency on `perch-github` or
`perch-app`. It knows about records and files, not about where records
come from.

```rust
pub enum Source { GitHub, Discord }

pub struct Message {
    pub source: Source,
    pub project: String,           // "slatedb"
    pub container: String,         // "slatedb/slatedb#812", later a channel name
    pub thread: Option<String>,    // a review thread, later a Discord thread
    pub id: String,                // the source's own id; re-ingest is a no-op
    pub reply_to: Option<String>,
    pub author: String,
    pub at: Timestamp,
    pub edited_at: Option<Timestamp>,
    pub body: String,
    pub url: String,               // permalink back to the source
    pub location: Option<String>,  // "src/manifest.rs:41" on a review comment
}
```

One record type for everything. A source that has more to say, a review's
verdict or a Discord reaction, says it in `body` or does not say it. The
model does not grow a field per source.

The store:

```rust
pub struct Knowledge { root: PathBuf }

impl Knowledge {
    pub fn open(root: PathBuf) -> Self;
    /// Append what is new, replace what changed, and re-render the files
    /// those records land in. Returns how many records were new or changed.
    pub fn ingest(&self, messages: &[Message]) -> Result<Ingested, Error>;
    /// Rebuild every rendered file and index from raw. For repairs and for
    /// when the renderer changes.
    pub fn rerender(&self) -> Result<(), Error>;
    pub fn write_skill(&self, config: &SkillConfig) -> Result<(), Error>;
}
```

`ingest` reads the raw file, merges by id, writes it back, then renders.
Reading a whole raw file per ingest is fine at this size and keeps the
store free of state between calls.

## Rendering

Boring on purpose, so a grep hit carries its own context on the line and
in the heading above it.

```markdown
# slatedb/slatedb#812: Fence the manifest on compaction

**rohan** opened 2026-09-14 14:02 [↗](https://github.com/slatedb/slatedb/pull/812)

<the description>

---

**chris** 2026-09-15 09:14 [↗](…)

Should this fence every write or only compactions?

**rohan** 2026-09-15 09:20 [↗](…) replying to chris 09:14

Only compactions. Writes go through the WAL fence already.

---

## Review: chris approved 2026-09-16 11:40 [↗](…)

**chris** 2026-09-16 11:38 [↗](…) on `src/manifest.rs:41`

This assumes the epoch is monotonic. Worth a comment.
```

Description first, then issue comments in time order, then each review
with its comments under it, in the order GitHub shows them. Bodies are
copied through as the markdown they already are, with one change: a
fenced code block that is not closed is closed, so a stray fence cannot
swallow the rest of the file.

`index.md` for a project lists its repositories, and for each source its
containers with a count and the newest timestamp. The root `index.md` lists
projects with their repositories, so that an agent can match a git remote
to a project, and when each was last updated.

Rendered files are written whole to a temporary name and renamed into
place. An agent reading mid-write sees the old file or the new one.

## The skill

Written to `~/.claude/skills/perch/SKILL.md` and
`~/.agents/skills/perch/SKILL.md`, the same content. Codex reads the
second path; Claude Code the first.

```markdown
---
name: perch
description: Discussion history for slatedb, perch from GitHub pull requests. Use when asked why something was decided, what was discussed, or for the context behind a change or a pull request.
---

Perch archives discussion about these projects as markdown under
`~/.perch/knowledge`. Start with `~/.perch/knowledge/index.md`, which maps
each project to its git remotes; match `git remote get-url origin` against
it to find the project for the repository you are in.

Layout, per project:

    <project>/index.md                   what is here and how current it is
    <project>/github/<owner>/<repo>/pulls/<n>.md
                                         description, comments, reviews
    <project>/raw/                       machine records; read the markdown instead

Search with `rg` before reading anything whole:

    rg -n "manifest fence" ~/.perch/knowledge/slatedb
    rg -n --glob 'pulls/8*.md' epoch ~/.perch/knowledge/slatedb

Every message carries its author, time and a permalink. Cite the permalink
when you rely on one. The files are updated while Perch runs, so read again
rather than remembering.
```

The description is the part that decides whether the skill loads, so it
names the projects and the phrases a person uses when they want history.
It is regenerated with the project list every time a project appears.

The body is prescriptive rather than exploratory. Every `ls` an agent has
to do is a chance to wander.

## GitHub

`perch-github` gains three fetches, all plain REST through the existing
client, all paginated to the end:

- `GET /repos/{o}/{r}/issues/{n}/comments` for the conversation.
- `GET /repos/{o}/{r}/pulls/{n}/reviews` for the verdicts and their bodies.
- `GET /repos/{o}/{r}/pulls/{n}/comments` for the line comments, which
  carry `pull_request_review_id`, `in_reply_to_id`, `path` and `line`.

and a `Discussion` type holding the four lists, description included.

A function in `perch-app`, beside the backend, turns a `PullRequest` and
its `Discussion` into `Vec<Message>`: `container` is `owner/repo#n`,
`thread` is the review id for a review comment, `reply_to` is
`in_reply_to_id`, `location` is `path:line`, and `id` is the source's
numeric id prefixed with its kind so a review and a comment cannot collide.
The description is a message whose id is `pr-<n>` and whose author is the
pull request's author.

The project is looked up from `config.toml`, falling back to the
repository name.

## Backend

One command and one event:

```rust
ArchiveDiscussion { repo_full_name: String, number: u64 },
```

```rust
Archived { repo_full_name: String, number: u64, added: usize },
```

`OpenPullRequest` and `OpenWorktree` send `ArchiveDiscussion` to
themselves after `ReviewReady`, on a spawned task, so the review paints
first. The task fetches, converts, ingests, writes the skill if the project
is new, and emits `Archived`. A fetch error becomes `Status` text and
nothing else. Offline, nothing is sent.

There is no ingest on startup and no timer. A pull request is archived
when it is opened, which is when the user is looking at it and when the
network is already being used for it.

## Config

```toml
[knowledge]
root = "~/.perch/knowledge"       # optional; this is the default
skills = true                     # optional; write the two SKILL.md files

[[knowledge.project]]
name = "slatedb"
repos = ["slatedb/slatedb", "slatedb/slatedb-go"]
```

`Config` in `perch-github` grows a `knowledge: KnowledgeConfig` with
serde defaults, so an existing `config.toml` keeps working.

## Build order

1. `perch-knowledge`: `Message`, `Knowledge::open`, `ingest` writing raw
   JSONL with merge by id, the pull request renderer, the two indexes,
   atomic writes. Tests against a temporary root: ingest then ingest again
   is a no-op, an edited body replaces, an unclosed fence is closed, the
   rendered file matches a fixture. Commit.
2. `perch-knowledge`: `write_skill` with the project list in the
   description. Test: the description names every project and no other.
   Commit.
3. `perch-github`: `Discussion` and the three fetches. A test against the
   existing mock server for pagination. Commit.
4. `perch-app`: the converter from `PullRequest` plus `Discussion` to
   messages. Tests on ids, threads, locations. Commit.
5. `perch-app`: config, the command, the event, the spawned task after
   `ReviewReady`, the status line. Commit.
6. README: a Knowledge section saying where the files are, that they are
   safe to delete, and how to point an agent at them. Commit.

Steps 1 through 3 are independent of each other.

## Acceptance

- Open a pull request with comments and a review. Under
  `~/.perch/knowledge/<project>/github/<owner>/<repo>/pulls/` there is one
  file with the description, every comment, and each review with its line
  comments under it, in order. Every message has a permalink that opens the
  right comment on github.com.
- `rg` for a phrase from a review comment finds the file and the line.
- Open the same pull request again: the raw file has the same number of
  lines, the rendered file is byte-identical, the status line says nothing
  was added.
- Edit a comment on github.com, open again: the rendered file has the new
  body and no copy of the old one.
- Both `SKILL.md` files exist, name the project in their description, and
  give the root that is in `config.toml`.
- Start Claude Code in a checkout of that repository and ask why a change
  in the pull request was made. It reads the archived file without being
  told the path.
- Delete everything under the project except `raw/`, run `rerender`: the
  files come back identical.
- Point `knowledge.root` at another directory, open a pull request: files
  land there, and the skill names the new root.
- Open a pull request offline: the review opens as before, nothing is
  archived, no error.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Do not add a database. Raw JSONL is enough and is greppable.
- Do not add a search command, an FTS index, or embeddings. Ripgrep is the
  search.
- Do not put anything in the record that only one source has. It goes in
  `body` or it goes nowhere.
- Do not render on a timer or on startup. Render inside `ingest`, for the
  files that changed.
- Do not write rendered files in place. Temporary name, then rename.
- Do not make `perch-knowledge` depend on `perch-github`. The converter
  lives in the app.
- Do not block `ReviewReady` on the discussion fetch.
- Do not fetch a repository's other pull requests. One pull request, when
  it is opened.
- Do not write the skill anywhere but the two paths above, and do not
  write it when `knowledge.skills` is false.
- Do not summarise. The files are the discussion, not a digest of it.
