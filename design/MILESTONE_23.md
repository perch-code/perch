# Perch: Milestone 23, Feedback and bug reports

## Goal

When Perch goes wrong on someone else's machine, the report arrives as a
screenshot of a toast: `rust-analyzer failed: the language server did
not answer in time`. That says what broke and nothing about why. Which
rust-analyzer, from which toolchain, how much memory it held, what the
log said before it gave up: all of it is on their disk, and getting it
means a back-and-forth in chat, or asking them to ask an agent.

After this milestone the report is filed from Perch, with that context
attached.

- **Send Feedback…** in the Perch menu opens a form: a title, what
  happened, and Bug or Idea.
- An error or warning toast gets a **Report** link that opens the same
  form, seeded with the toast's text.
- Perch collects what it knows about itself, its environment and the
  last few minutes of its log, and redacts it.
- For a bug, Claude or Codex can look into it first, read only and
  offline, and write notes on what it checked and what it thinks the
  cause is.
- You read and edit the whole issue before anything is sent. **File
  issue** creates it on `criccomini/perch` with your token, and it opens
  in Perch as an issue.

Perch also starts writing its log to a file. Today it goes to stderr,
which a Finder launch throws away.

## Non-goals

- Filing without the preview, or filing on its own: no crash reporter,
  no "send automatically", no telemetry of any kind.
- Screenshots or other attachments. GitHub's API has no upload for issue
  attachments. Paste one into the issue on github.com afterwards.
- Reporting a crash on the next launch. The panic is in the log (see
  [The log file](#the-log-file)), and a report filed after relaunching
  includes it. Detecting the crash and offering the form is a
  follow-up.
- Letting the agent file, search GitHub, or reach the network. It gets
  Ask's sandbox and writes notes, and Perch does the rest.
- Shelling out to `gh`, as in Milestone 21.
- Reports to any repository but Perch's own.
- Editing or following up on an issue after it's filed. It's an issue
  item in Perch, and Conversation does that already.

## Behaviour

### Where it is

- **Send Feedback…** goes in the Perch menu after Open…, with no key
  equivalent. It has the app's target as New Window and Open… do, so it
  works with no window open: it opens one first.
- A toast of `Tone::Error` or `Tone::Warning` gets **Report** between its
  text and its close button. Clicking it dismisses the toast and opens
  the form with Kind set to Bug, the title set to the toast's text
  (first line, up to 80 characters), and the toast recorded as the
  problem being reported.
- Toasts go after a few seconds, so Perch keeps the last 20 error and
  warning messages of the session, with their times, in memory. Every
  report includes them (see [What Perch collects](#what-perch-collects)),
  so one filed from the menu after the toast has gone still has it.

### The form

A box over the window, as Open… is, wider, with:

- **Kind**: Bug · Idea. Bug from a toast, Idea from the menu.
- **Title**, one line.
- **What happened**, the composer's box with no destination and no `@`
  completion. Placeholder for Bug: `What did you do, what did you
  expect, and what happened instead?` For Idea: `What would you like
  Perch to do?`
- **Let Claude look into it** (or Codex, the model menu's pick), a
  checkbox. On for Bug, off for Idea. Off and disabled, with the reason
  as its tooltip, when neither CLI is installed.
- **Preview…** and **Cancel**.

`Escape` or Cancel closes the box. What was typed stays until Perch
quits or the issue is filed, so reopening the box from the menu brings
the draft back. Opening it from another toast's Report while a draft
exists asks `Replace the draft you started?`

### Preview

**Preview…** runs, in order:

1. **Collecting…** Perch gathers its context, at once, with no network
   but the rate-limit figure it already has.
2. **Looking into it…**, when the box is ticked. The agent run, with a
   **Stop** beside the label. See [The agent's notes](#the-agents-notes).
3. **Searching…** Up to three open issues on `criccomini/perch` whose
   titles match the report's, from one `search_issues` call
   (`repo:criccomini/perch is:issue is:open` and the title's words). A failed or offline search
   shows none and says nothing.

Then the box becomes the preview:

- The title, still editable.
- The issue body as it will be sent, in one editable text box, in the
  shape given in [The issue](#the-issue). Edits here are what's sent:
  delete a section and it's gone.
- **Similar issues**, when the search found any: each row is the
  number, title and age, opens the issue in Perch on click, and has
  **Comment there instead**.
- **File issue**, **Back** to the form, and **Cancel**.

The box can be closed during step 2. The run carries on, and when it
ends a toast says `Your report is ready to review`, which reopens the
preview on click. Stop, or a run that fails or takes more than ten
minutes, gives a preview with no agent notes and a line saying why.

### Filing

**File issue** posts `POST /repos/criccomini/perch/issues` with the
title, the body and a label, `bug` or `enhancement` by Kind. GitHub
drops labels from someone without push access, and that's fine.

**Comment there instead** posts the body, without the user's title, as a
comment on that issue with `create_issue_comment`.

What's sent is the preview's title and text, word for word. Before
either, Perch runs the redaction again over the edited text (see
[Redaction](#redaction)), so a secret pasted into the preview is caught
too. If that changes anything, nothing is sent: the preview takes the
redacted text and says `Redacted 1 value you added. Check it and file
again.`

On success the draft is cleared, the box closes, a toast says `Filed
criccomini/perch#123` (or `Commented on …`), and the issue opens through
`open_subject`, so it's an item with its Conversation, and its replies
reach the Inbox as notifications.

On failure the box stays with what was typed:

- **The token can't write issues there**: a `403` or `404`, which is
  what a fine-grained token gets on a repository its owner didn't pick.
  The line says `Your token can't open issues on criccomini/perch.` and
  offers **Open on GitHub**: it copies the body to the clipboard and
  opens `https://github.com/criccomini/perch/issues/new?title=<title>`.
  The body never goes in the URL.
- **Offline**: `Offline.` Nothing is attempted. The draft keeps.
- Anything else: GitHub's message.

### What Perch collects

Plain facts Perch has or can get with a local command, each under a
heading in the Diagnostics section:

- **Perch**: the package version and the commit it was built from, with
  `-dirty` when the tree had changes, from a `build.rs` (see
  [Backend](#backend)); debug or release; whether it was launched from
  Finder or a terminal (a parent of `launchd`, or not).
- **System**: `sw_vers` product version and build, the architecture,
  each screen's size and backing scale, and the window's size.
- **Agents**: the path and `--version` of `claude` and `codex` as Perch
  found them, or `not found`.
- **Language servers**: for each server Perch has running or tried to
  start, its `Status`, the binary `find_server` picked and from where
  (the checkout's toolchain, `PATH`, or cargo's bin directory), its
  `--version`, the checkout's toolchain pin, and its pid, resident
  memory and uptime. Then the count and total resident memory of
  `rust-analyzer` processes Perch didn't start.
- **GitHub**: the hosts, where the token came from (`GITHUB_TOKEN`,
  `GH_TOKEN`, `gh auth token` or config, never the value), the last
  poll's outcome and time, and the rate limit's remaining and reset as
  Perch last saw them.
- **Config**: `~/.perch/config.toml` with `token` taken out and each
  `[[repo]]` reduced to whether it has `github`, how many `roots`, and
  whether it sets `trust` or `[repo.work]`. `[agent]` and `[work]` as
  they are.
- **The open item**, if any: its kind, whether it has a checkout and
  whether that's under a root, and its language server's status. Not its
  repository or number (see [Redaction](#redaction)).
- **Recent problems**: the session's last 20 errors and warnings, oldest
  first, with times, the reported one marked.
- **Log**: the last 300 lines of `perch.log`.

Each command gets five seconds. One that doesn't answer is reported as
`timed out`, and the rest go on.

### The agent's notes

The run has Ask's rules: read only, no network, the scrubbed
environment, nothing reaches GitHub. Its working directory is the open
item's checkout when there is one, since a language server's trouble is
usually that checkout's toolchain, and `~/.perch` otherwise. It isn't an
agent run, for the reason Milestone 21's description run isn't one: no
thread, no card, not in the Agents dropdown.

The prompt gives the Kind, title and what happened, the collected
context, and the path of `~/.perch/logs`. It asks the agent to:

- find out why the reported problem happened on this machine, by reading
  Perch's logs and config and running read-only commands (`ps`, `which`,
  `--version`, `rustup show`, and the like);
- read build and toolchain files in the checkout (`rust-toolchain.toml`,
  `Cargo.toml`, `.cargo/config.toml`) when they bear on it, and not the
  project's source;
- answer in GitHub Markdown, under 400 words, in three parts: **Checked**
  (each command and what it showed), **Likely cause**, and **Not
  sure**, for what it couldn't confirm.

A command the sandbox refuses is the agent's to note and move past. The
collected context doesn't depend on the run, so a run that finds nothing
still leaves a useful report.

### The issue

````markdown
<What happened, as typed>

---

Perch 0.1.0 (a1b2c3d) · macOS 15.6 (24G84) arm64 · Claude Code
2.1.230 · Codex 0.160.0

<details><summary>Agent notes (Claude, unverified)</summary>

…

</details>

<details><summary>Diagnostics</summary>

…

</details>

<details><summary>Log (last 300 lines)</summary>

```text
…
```

</details>

Filed from Perch's Send Feedback.
````

The summary line is always there. The agent notes are there when the run
produced some. For an Idea, Diagnostics and Log are left out unless the
box was ticked; the summary line stays.

GitHub caps a body at 65,536 characters. Over that, the log is cut from
its oldest end until it fits, and says how many lines it lost.

### Redaction

Run over everything collected and the agent's notes, before the
preview, and again over the edited text before sending:

- The exact token Perch holds, and the value of every variable in
  Perch's environment that `is_scrubbed_env` matches, including the ones
  it keeps for the CLIs: `‹redacted›`.
- Token-shaped strings: `gh[opusr]_…`, `github_pat_…`, `sk-ant-…`,
  `sk-…`, `xox[abp]-…`, `AKIA…`, and `Authorization:` and `Bearer`
  values: `‹redacted›`.
- `$HOME` as `~`, and the user name wherever else it appears in a path:
  `‹user›`.
- Email addresses, except GitHub's `noreply` ones: `‹email›`.
- Every repository named in the config or among Perch's items, other
  than `criccomini/perch`, as `owner/repo` or in a path or URL:
  `‹repo›`. A private repository's name is not Perch's to publish, and
  Perch doesn't track which are private. The user can type one back in
  the preview.

The preview shows `Redacted 3 values` over the text when any were. Each
pass is a pure function with its own tests.

### The log file

Perch writes its log to `~/.perch/logs/perch.log` as well as stderr,
with the same filter and no colours. It rotates daily and keeps 7 files.
A panic hook logs the panic message and location at `error` before the
default hook runs, so a crash leaves its cause in the file.

## Model

- `AppState` gains `feedback: RwSignal<Option<FeedbackDraft>>`, one for
  the app, not per item: `FeedbackDraft { kind, title, body, diagnose,
  seed, step, context, notes, preview, similar, error, generation }`.
  `seed` is the toast being reported. `step` is `Editing`, `Collecting`,
  `Diagnosing`, `Searching`, `Preview` or `Filing`.
- `AppState` gains `recent_problems`, a ring of 20 `(time, text, tone)`,
  pushed by `notify` for errors and warnings.
- Nothing is stored in the database. No schema change.

## Backend

- `perch-app/build.rs` sets `PERCH_COMMIT` from `git rev-parse --short
  HEAD` and `-dirty` from `git status --porcelain`, or `unknown` outside
  a checkout. It reruns when `git rev-parse --git-path HEAD` and the ref
  it names change, which works in a worktree, where `.git` is a file.
- `tracing-appender` for the file, non-blocking, its guard held by
  `main`.
- `perch-github`: `create_issue(repo, title, body, labels)`, parsing the
  response as `subject_kind` parses an issue.
- `backend/feedback.rs`: `collect(ctx, item_id, problems) ->
  FeedbackContext`, `redact(text, secrets) -> (String, usize)`,
  `render(draft) -> String` and the agent run, spawned directly with
  `perch_agent::Run` and kept for Stop, as `open_pull::describe` is.
- The commands `PrepareFeedback { kind, title, body, diagnose, seed,
  item_id, backend, model, effort }`, `StopFeedbackDiagnosis`, and
  `FileFeedback { title, body, label, comment_on: Option<u64> }`, with
  the events `FeedbackStep`, `FeedbackReady { context, notes, preview,
  similar }`, `FeedbackFiled { number, comment }` and `FeedbackFailed {
  error, can_open_on_github }`.
- The target repository is a constant, `criccomini/perch`.

## Harness

Headless tests in the style of the ones they sit beside, with GitHub's
paths answered by the test server.

- Entry points: Send Feedback… opens the box with Idea; Report on an
  error toast opens it with Bug, the toast's text as the title, and the
  toast gone; a success toast has no Report; reopening keeps the draft;
  a second Report asks before replacing it.
- Recent problems: holds 20, drops the oldest, skips success toasts.
- Collection: each section renders from a fixed context; a command that
  hangs is `timed out` after its limit and the others still appear.
- Redaction: the held token, a scrubbed variable's value, each token
  shape, `$HOME`, the user name in a path, an email but not a `noreply`
  one, and a configured repository name in prose, a path and a URL are
  each replaced and counted; `criccomini/perch` isn't. A send whose text
  the second pass changes posts nothing and puts the redacted text in the
  preview.
- The issue: summary line always; notes only when present; an Idea
  without diagnosis has neither Diagnostics nor Log; an oversized log is
  cut from its oldest end with the count.
- The agent run: its prompt has the user's text, the context and the
  logs path; it runs with Ask's tools in the open checkout, or
  `~/.perch` with none; it records no run; Stop and a timeout give a
  preview without notes and say why.
- Similar issues: the search query; three at most; Comment there instead
  posts to that number; a failed search shows none.
- Filing: posts the preview's title and body, unchanged, and the label
  to `criccomini/perch`;
  opens the issue as an item; clears the draft. A `403` offers Open on
  GitHub, which copies the body and puts only the title in the URL.
  Offline sends nothing and keeps the draft.
- The log file: written under `$PERCH_HOME/logs`, with no escape codes;
  a panic in a test thread lands in it.

## Build order

1. The log file, the panic hook and `build.rs`. Commit. (Worth landing
   on its own: the log is useful before any of the rest exists.)
2. `collect`, `redact` and `render`, with their tests. Commit.
3. `create_issue`, `FileFeedback`, and the form and preview without the
   agent or the search: Send Feedback…, Report on toasts, recent
   problems, Open on GitHub. Commit.
4. The agent's notes: the run, Stop, the timeout, the toast when the box
   was closed. Commit.
5. Similar issues and Comment there instead. Commit.
6. README: Setup › GitHub's table gains `File feedback | Issues: write on
   criccomini/perch`, with a line that a fine-grained token can't get it
   for someone else's repository and Perch falls back to github.com.
   Agent safety gains a line that the feedback run is an Ask run and
   nothing it writes is sent until you file. Data gains `logs/`. A short
   **Feedback** section says what a report includes and what's
   redacted. Commit.

## Acceptance

These file real issues on `criccomini/perch`. Close them afterwards.

- Put a `rust-analyzer` on `PATH` that sleeps forever, remove the
  toolchain's own, and open a Rust pull request. The `rust-analyzer
  failed` toast shows with Report. Press it: the box has Bug and the
  toast's text. Tick the box, Preview. The Diagnostics name the sleeping
  binary and that it came from `PATH`; the agent's notes say so too and
  suggest the toolchain's. Nothing in the body has your home directory,
  user name, token or any repository name but Perch's.
- Wait for that toast to go, then open Send Feedback… from the menu and
  Preview: the toast is under Recent problems.
- File it: the issue is on `criccomini/perch` with the `bug` label,
  opens in Perch as an issue item, and the box is gone. Reply on
  github.com: the reply reaches the Inbox.
- Launch Perch from Finder, use it for a minute and quit:
  `~/.perch/logs/perch.log` has the session.
- Paste your token into the preview's text and press File issue: nothing
  is filed, and the preview shows `‹redacted›` where it was, with the
  notice. Press File issue again: the issue matches the preview.
- With a fine-grained token that doesn't include `criccomini/perch`,
  file: the `can't open issues` line shows, and Open on GitHub opens the
  new-issue page with the title, the body on the clipboard.
- Start a report with the box ticked, close the box while it's looking:
  the toast says the report is ready, and clicking it opens the
  preview.
- File a second report with the first one's title: the first one is
  under Similar issues, and Comment there instead adds a comment to it.
- Turn the network off and press File issue: `Offline.`, and the draft
  is intact.

## Things the implementer should not do

- Don't send anything to GitHub without the File issue or Comment there
  instead button, and don't send text the user hasn't seen in the
  preview.
- Don't put the body, or anything collected, in a URL.
- Don't give the agent the token, the network, or a way to file.
- Don't collect the user's project source, their notifications, or the
  contents of the database.
- Don't record the feedback run as an agent run, or add a `RunOwner`
  kind for it.
- Don't call `gh`.
- Don't run `cargo fmt --all`. Format only new files.
