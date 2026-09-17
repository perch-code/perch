# Perch: Implementation Plan, Milestone 1

## Goal

A native macOS desktop app, written entirely in Rust, that shows the user's
GitHub notifications inbox. It should look and behave like the GitHub
notifications web UI (github.com/notifications), but read-only: no marking
as done, no saving, no unsubscribing, no clicking through to the PR or issue.
Just list them, filter them, sort them, group them, search them.

This is the first milestone of a larger read-only code review tool. Nothing in
this milestone should paint us into a corner for later work (an embedded
editor, LSP, PR diffs, LLM review), but nothing here should build for it
either. Keep it small.

## Non-goals for this milestone

- Any mutation: mark as read, mark as done, save, unsubscribe.
- Opening a notification. Rows are not clickable. No detail pane.
- The "Saved" and "Done" views. GitHub's REST API does not expose these
  cleanly. Show "Inbox" only.
- Discussions, releases, commits, security alerts, CI failures as first-class
  rows. Show them if they come back from the API, but only PRs and issues get
  the full treatment (state icon, number, author avatar).
- Windows and Linux. Build on macOS. Don't do anything that breaks other
  platforms, but don't test them.
- Settings UI. Config is env vars and a TOML file.
- Auto-update, signing, notarization, DMG packaging.

## Stack

All Rust. Pin `floem` to a git rev and note it in `Cargo.toml`.

| Concern | Crate |
|---|---|
| UI | `floem` (git rev, no `editor` feature yet) |
| GitHub API | `octocrab` |
| HTTP for avatars | `reqwest` (rustls, no openssl) |
| Image decode | `image` (png, jpeg only) |
| Async | `tokio` (multi-thread runtime on a background thread) |
| Local cache | `rusqlite` (bundled) |
| Serialization | `serde`, `serde_json` |
| Time | `jiff` or `time` for relative timestamps |
| Config | `toml`, `directories` |
| Errors | `thiserror` in library code, `anyhow` in `main` |
| Logging | `tracing`, `tracing-subscriber` |

Do not add a crate for fuzzy search, virtual lists, or state management.
Floem ships `virtual_list` and reactive signals. Search is substring match.

## Repository layout

```
perch/
  Cargo.toml              workspace
  crates/
    perch-github/         GitHub client, models, cache. No UI deps.
    perch-app/            Floem app. Binary target `perch`.
  IMPLEMENTATION_PLAN.md
  README.md
```

`perch-github` must compile and test without Floem. That's the boundary we
keep for every later milestone: data and IO crates never depend on the UI.

## Authentication

Resolve a token in this order and stop at the first hit:

1. `GITHUB_TOKEN` env var
2. `GH_TOKEN` env var
3. `gh auth token` subprocess, trimmed stdout, if exit status is zero
4. `token` key in `~/Library/Application Support/perch/config.toml`

If nothing resolves, the app opens and shows one centered message: "No GitHub
token found. Set GITHUB_TOKEN or run `gh auth login`." Do not build a login
flow.

Note for the implementer: GUI apps launched from Finder don't inherit shell
env vars. Step 3 exists because of that. Step 4 exists for people without
`gh`.

The token needs the `notifications` scope (classic) or "Notifications: read"
(fine-grained). Mention this in the README.

## Data model

Live in `perch-github`. Mirror the REST API closely so the cache layer is
trivial.

```rust
pub struct Thread {
    pub id: String,
    pub unread: bool,
    pub reason: Reason,
    pub updated_at: Timestamp,
    pub last_read_at: Option<Timestamp>,
    pub subject: Subject,
    pub repository: Repository,
}

pub enum Reason {
    Assign, Author, Comment, CiActivity, Invitation, Manual, Mention,
    ReviewRequested, SecurityAlert, StateChange, Subscribed, TeamMention,
    Other(String),
}

pub struct Subject {
    pub title: String,
    pub kind: SubjectKind,
    pub url: Option<String>,             // API URL of the issue/PR
    pub latest_comment_url: Option<String>,
}

pub enum SubjectKind {
    Issue, PullRequest, Discussion, Commit, Release, CheckSuite,
    RepositoryVulnerabilityAlert, Other(String),
}

pub struct Repository {
    pub full_name: String,               // "slatedb/slatedb"
    pub owner_avatar_url: String,
}

/// Fetched separately, per thread. Only for Issue and PullRequest.
pub struct SubjectDetail {
    pub number: u64,
    pub state: SubjectState,
    pub author_login: String,
    pub author_avatar_url: String,
}

pub enum SubjectState {
    IssueOpen, IssueClosed, IssueNotPlanned,
    PrOpen, PrDraft, PrMerged, PrClosed,
}
```

`Reason` and `SubjectKind` must deserialize unknown strings into `Other`
rather than failing. GitHub adds reasons without notice.

## GitHub API usage

### Listing threads

`GET /notifications` via `octocrab.activity().notifications().list()`.

- Always pass `all=true`. The web UI's Inbox shows read and unread threads.
  Unread is a filter, not a query.
- Pass `per_page=50` and paginate until an empty page or 10 pages, whichever
  comes first. 500 threads is more than the web UI shows.
- Honor `X-Poll-Interval` from the response header. Default to 60 seconds.
  Never poll faster than the header says.
- Send `If-Modified-Since` with the `Last-Modified` from the previous
  response. A 304 means nothing changed; keep what we have.
- On 401, show the "no token" message with "token rejected" wording.
- On 403 with rate limit headers, back off until `X-RateLimit-Reset` and
  show "rate limited until HH:MM" in the status bar.

### Subject details

For each thread whose `subject.kind` is `Issue` or `PullRequest`, fetch
`subject.url` to get number, state, author, and for PRs the `draft` and
`merged` flags. Issues use `state` plus `state_reason` (`not_planned`).

This is one request per thread. Cache by `(thread.id, thread.updated_at)`.
A thread whose `updated_at` hasn't changed reuses the cached detail. On a
cold start with 100 threads that's 100 requests, once. Fine under the 5000
per hour limit. Fetch details concurrently, 8 at a time, and render rows
before details arrive. A row without detail shows a neutral icon and no
number until the detail lands.

The number is parseable from the tail of `subject.url` without a fetch.
Do that immediately so rows show `#2097` on first paint.

### Avatars

Fetch `author_avatar_url` with `?s=40` appended, decode with `image`, and
cache the decoded bytes on disk under the app data dir keyed by URL hash.
Load from disk first. Never block the UI thread on this.

Show one avatar per row: the subject author. The web UI shows up to three
participants but that needs the timeline API. Skip it.

## Local cache

SQLite at `~/Library/Application Support/perch/perch.db`. Two tables.

```sql
CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    updated_at TEXT NOT NULL,
    json TEXT NOT NULL          -- serialized Thread
);

CREATE TABLE subject_details (
    thread_id TEXT NOT NULL,
    updated_at TEXT NOT NULL,   -- thread.updated_at at fetch time
    json TEXT NOT NULL,         -- serialized SubjectDetail
    PRIMARY KEY (thread_id, updated_at)
);
```

On startup, load threads from SQLite and render immediately, then refresh
from the API. The app should show something within a second even offline.

Threads that disappear from the API response are deleted from the cache.
GitHub removes done and unsubscribed threads from the list, and so do we.

## UI specification

Match the screenshot of the web UI. Single window, default 1280x800, minimum
900x600. Light theme only for this milestone, but read colors from one
`theme.rs` so dark mode is a later diff.

### Layout

Two columns. Left sidebar fixed at 220px. Right column fills the rest.

**Sidebar, top to bottom:**

- Views: `Inbox` with unread count badge. `Saved` and `Done` are listed but
  greyed out and inert.
- "Filters" heading, then rows: `Assigned`, `Participating`, `Mentioned`,
  `Team mentioned`, `Review requested`. Each shows a count badge when its
  count is nonzero. Clicking selects that filter. Clicking again clears it.
- "Repositories" heading, then one row per repo with threads, with a count
  badge. Clicking filters to that repo. Clicking again clears.

**Right column, top to bottom:**

- Toolbar: segmented `All | Unread` control, search text input, `Sort by`
  dropdown, `Group by` dropdown.
- Thread list, grouped or flat depending on `Group by`.
- Status bar: last refreshed relative time, and any error or rate limit
  message. Keep it one line.

### Filter semantics

Filters compose with AND. All operate on the in-memory thread list; no
refetch when a filter changes.

| Sidebar filter | Matches when |
|---|---|
| Assigned | `reason == Assign` |
| Participating | `reason` in `Author, Comment, Mention, TeamMention, ReviewRequested, Assign` |
| Mentioned | `reason == Mention` |
| Team mentioned | `reason == TeamMention` |
| Review requested | `reason == ReviewRequested` |
| Repository | `repository.full_name` equals the selected repo |
| Unread | `unread == true` |
| Search | `subject.title` or `repository.full_name` contains the query, case-insensitive |

Only one of the five reason filters can be active at a time. Repo, unread,
and search stack on top of it.

### Sort options

- Newest to oldest by `updated_at` (default)
- Oldest to newest

### Group options

- Repository (default). One section per `repository.full_name`, sections
  ordered by most recent thread in the group. Section header shows the repo
  name. Threads within a group follow the sort order.
- Date. Sections: Today, Yesterday, This week, Earlier.
- None. Flat list.

### Row

Left to right, fixed heights, 56px per row:

1. Unread dot, 8px, blue. Empty space if read.
2. State icon, 16px. Issue open: green circle. Issue closed: purple circle
   with check. Issue not planned: grey circle with slash. PR open: green
   pull-request glyph. PR draft: grey pull-request glyph. PR merged: purple
   merge glyph. PR closed: red pull-request glyph. Unknown or not yet
   loaded: grey hollow circle. Other subject kinds: grey dot.
   Draw these as simple vector shapes in Floem. Don't pull in an icon font.
3. Two text lines. Top line, small grey: `repo/name #1234`. Bottom line,
   regular: subject title, single line, ellipsized.
4. Reason badge, right-aligned, small grey text: the reason in the web UI's
   wording. `subscribed`, `mention`, `author`, `commented`,
   `review requested`, `team mentioned`, `assigned`, `ci activity`,
   `state change`, otherwise the raw string.
5. Author avatar, 20px circle. Placeholder grey circle until loaded.
6. Relative time, right-aligned, grey: `42 minutes ago`, `5 hours ago`,
   `yesterday`, `2 days ago`, `last week`, `2 weeks ago`, `3 weeks ago`,
   then `Jan 3` style dates beyond a month.

Rows do nothing on click. No hover state is required, but a subtle
background on hover is fine if cheap.

### Empty and error states

- No token: centered message, sidebar hidden.
- Loading with empty cache: centered "Loading notifications…".
- Zero threads after filtering: centered "No notifications match."
- API error with a populated cache: show cached data, error in status bar.

## Threading model

Floem's event loop owns the UI thread. Run one `tokio` multi-thread runtime
on a separate thread, started in `main` before the Floem app.

Communication is one direction each way:

- UI → background: an `mpsc::UnboundedSender<Command>` with variants
  `Refresh`, `FetchDetail(thread_id)`, `FetchAvatar(url)`.
- Background → UI: `floem::ext_event::create_signal_from_channel` or an
  equivalent, delivering `Event` variants `ThreadsLoaded(Vec<Thread>)`,
  `DetailLoaded(thread_id, SubjectDetail)`, `AvatarLoaded(url, Image)`,
  `Status(String)`, `Error(String)`.

All app state lives in Floem signals in one `AppState` struct: threads,
details map, avatars map, active filters, sort, group, search, status.
Derived views (filtered and grouped lists) are memos over those signals.
Nothing in the UI crate touches `octocrab` or `rusqlite` directly.

## Config file

`~/Library/Application Support/perch/config.toml`, optional:

```toml
token = "ghp_..."          # only if env vars and gh are unavailable
poll_interval_secs = 60    # floor; X-Poll-Interval may raise it
```

## Build order

Do these in sequence. Each step ends with something runnable.

1. Workspace, two crates, `perch` binary opens an empty Floem window with
   the two-column skeleton and static placeholder text. Commit.
2. `perch-github`: token resolution, `Thread` model, `list_threads()`
   with pagination. A `perch-github` example binary that prints threads
   as JSON. Unit tests for `Reason` and `SubjectKind` deserialization
   including unknown values. Commit.
3. Wire the background runtime and channels. Real threads render in a flat
   list with title, repo, reason, relative time. No icons, no avatars.
   Commit.
4. Sidebar filters, All/Unread toggle, search, sort. All in-memory. Commit.
5. Group by repository and date. Commit.
6. SQLite cache. Cold start renders from cache before the network. Commit.
7. Subject detail fetch with concurrency limit and caching. State icons and
   numbers. Commit.
8. Avatars with disk cache. Commit.
9. Polling with `X-Poll-Interval` and `If-Modified-Since`. Status bar.
   Rate limit handling. Commit.
10. README with token setup and build instructions. Commit.

## Acceptance criteria

- `cargo build --release` produces a single binary with no runtime deps
  beyond `gh` (optional) and network.
- Launch with a valid token shows the same threads as github.com/notifications
  in the same default order, grouped by repository.
- Each of the five sidebar filters, the repo filter, unread, search, both
  sorts, and all three groupings produce the expected subset and order.
- Kill the network, relaunch: cached threads render, status bar shows the
  error.
- Idle memory under 100 MB with 200 threads and avatars loaded.
- `cargo test` passes in both crates. `cargo clippy` is clean.

## Things the implementer should not do

- Don't add a settings screen, menu bar items, or keyboard shortcuts.
- Don't make rows clickable or add a detail pane.
- Don't use GraphQL. REST is enough for this milestone.
- Don't fetch participants, timelines, or comments.
- Don't add dark mode. Do keep colors in one file.
- Don't add the Floem `editor` feature or any tree-sitter, LSP, or git
  crates. Those come in milestone 2.
