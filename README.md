# Perch

Perch is a native macOS coding and review tool. It combines agent-driven
worktrees, code browsing and LSP support, GitHub reviews and issues, and local
human/agent conversations in a compact Rust application built with Floem and
Kenn Forge.

## GitHub inbox

Perch builds its inbox from GitHub's REST notifications list. It preserves the
API's order and unread state, and caches the latest response locally so the
sidebar can paint immediately. Perch requests `all=true` so read threads remain
visible alongside unread ones.

Polling respects GitHub's `X-Poll-Interval` and paginates the full response.
Perch performs an unconditional reconciliation at startup, on manual refresh,
and on the first scheduled poll after 15 minutes; polls between reconciliations
use `Last-Modified`. The full fetch repairs the sidebar when GitHub's validator
does not change with the notification list.

- Viewing a pull request or an issue marks its GitHub thread read, from
  whichever row it was opened: the notification's, or the item's in All or
  Authored. Read threads remain in the inbox. While the thread is unread, the
  item's row is bold with the unread dot, as the notification's row is.
  GitHub's list can go on reporting a thread unread for a while after it has
  taken the request, so a read made in Perch holds for that revision of the
  thread until newer activity arrives or it is marked unread; a thread marked
  unread on github.com for the same revision is not seen.
- **Mark Unread**, on the notification's row or the item's, restores the
  unread reminder in Perch for the current update. Viewing it clears the
  reminder; later GitHub state takes precedence. GitHub's public API has no
  mark-unread operation, so this does not change github.com.
- **Done** marks the thread done on GitHub and remembers that exact update in
  Perch, so GitHub's `all=true` history cannot put it back. Newer activity
  clears the local mark and lets the thread return.
- **Unsubscribe** ignores future activity on the thread, marks its current
  notification done, and remembers it the same way.
- Pull requests and issues open inside Perch. Other notification types remain
  first-class inbox rows and open on GitHub.

GitHub's public REST notification payload does not expose whether a thread is
currently in the web Inbox or Done, and `all=true` can include Done history.
Perch can therefore keep its own Done actions stable, but cannot reliably
mirror Done or Restore actions performed on github.com.

These actions affect only the notification or its local reminder. They do not
archive or delete a Perch item, worktree, or local conversation.

Perch does not crawl a repository's open items or history. A pull request or
issue is fetched when a notification is opened or when the user opens it by
repository and number. Its current data and discussion may be cached locally;
closed history is never backfilled.

## Timeline

Conversation is the item's description, what was said about it, and what was
done to it, in one column in time order. The doing comes from GitHub's own
timeline: commits, labels added and removed, assignments, review requests and
dismissals, renames, milestones, force pushes, references from other issues
and pull requests, and the opening, merge or close. Each is one line saying
who did it and when, with a label in its own colours, a commit by its short
sha and subject, and a reference by where it points. A commit is signed with
the name git recorded, since it has no account behind it.

What Perch already draws as a card — a comment, a review, a line comment — is
not repeated as a line, and what GitHub sends but does not itself show, such
as `mentioned` and `subscribed`, is left out. A kind Perch has no words for is
dropped rather than guessed at. Ten pages are read for one item, a hundred
lines to a page.

## Reactions

Each GitHub entry on Conversation shows the reactions on it under its text:
the description, every comment, and every review summary and line comment. A
pill is one reaction and its count, blue when yours is among them. Clicking a
pill adds or removes yours. The face at the right of an entry's byline, and
the one after its pills, open GitHub's picker: all eight in a row, yours
washed blue, and picking one of yours takes it back. A reaction is shown as
soon as it is clicked and put back if GitHub refuses it.

Reactions are read with the discussion through GitHub's GraphQL API, a
hundred entries to a request, and left the same way. An entry cached by an
older build of Perch has no reactions until it is fetched again.

## Posting

A composer's box opens five lines tall, the size GitHub gives a comment, and
grows with what is typed. The grip at the foot of the box drags it taller or
shorter, between two lines and about twenty-four; a box that has been dragged
keeps that height whatever is typed into it, and every composer opens at it
until Perch is restarted.

Typing `@` in a composer, or in the Submit review summary, puts a list under
it, as GitHub's comment box does. People come first: the item's author and
whoever has written on it, then anyone else in the repository GitHub finds
for the letters typed, never you. The worktree's files follow, ranked as
`Cmd-P` ranks them. The arrows or `Ctrl-N` and `Ctrl-P` move the pick, `Enter`
or `Tab` takes it, a click takes a row, and `Escape` puts the list away until
the next `@`. A person goes in as `@login`. A file goes to an agent as
`@path`, as Claude Code and Codex take one, and to GitHub as `` `path` ``,
since `@` there mentions someone and a code span links to Files.

Every composer in Perch has a destination: Claude, Codex, or GitHub, picked
from the same menu as the model. What goes to an agent stays in Perch. What
goes to GitHub goes one of two ways.

On a line of a file, or under a thread that is on GitHub, a message to GitHub
is a draft: it is kept in Perch's cache the moment it is sent, drawn with a
`Draft` pill, marked in the gutter in GitHub's blue, and can be edited in
place or deleted on hover. Drafts survive a restart and being offline, and
github.com does not show them. **Submit review** lists them above the summary,
counts them on the button, and posts them with the verdict: a comment on the
head rides in the review, a comment written against an older commit is posted
against that commit, and a reply is posted under its thread. A `Comment`
review with drafts needs no summary. A draft that GitHub refuses, or that
cannot be matched to what GitHub returns, stays a draft and the form says so.
GitHub's API takes a review comment only on the lines of the pull request's
diff, a changed line or one of the three around it, though github.com takes
one on any line, and one comment it refuses fails the whole review. So a
comment on the head that is not inside one hunk of the diff, first line and
last, is kept out of the review: the rest posts, and it stays a draft, named
by its lines on the form.

At the foot of Conversation, a message to GitHub is an issue comment and
posts at once, after one confirmation naming the pull request.

GitHub's review threads are drawn beside Perch's own with the same cards, in
both Files and Conversation, so one thread can hold a reviewer's comment, your
reply to them, and an agent's private take on both. A thread looks as it does
on github.com: on Conversation, a box headed by its file with the lines it is
on under the header; in Files, a box under those lines headed "Comment on line
R12". Each message sits under its author's face and name, with **Author** by
whoever opened the pull request, and the thread ends in a **Follow up** box and
a foot with **Resolve**. A reviewer's comment offers **Reply** and **Ask about
this** under the pointer. In such a thread, what an agent said and what you
asked it are labelled **Perch only**.
An agent's answer never posts on its own: **Post to GitHub** under the pointer
puts its text in a composer for you to edit and send, and nothing records
where it came from. It goes the same two ways as anything else: a draft under
a thread with lines or a place on GitHub, an issue comment under one of
Perch's own, such as a question asked at the foot of Conversation. A review's
findings on an open pull request arrive as drafts already (see Worktree
reviews), and post only with the review.

A thread ends in **Resolve**, or **Unresolve** once it is resolved. On a
thread that is on GitHub the button is there wherever GitHub says you may
use it, and the click changes github.com at once, with no confirmation, as
the button there does. The thread shows the change before GitHub answers
and goes back if GitHub refuses. On a thread of Perch's own, a question to
an agent and its answers, the button is always there and the change stays
in Perch. A review finding has **Dismiss** and **Reopen** instead.
Resolved, a thread folds, in Files to one row and on Conversation to its
header, as one resolved on github.com arrives; **Show resolved** opens it
again and **Hide resolved** folds it. Conversation and Files each keep their
own, as github.com's tabs do, and the chevron on a thread's header folds any
thread.

## Authentication

Perch calls GitHub's API directly. It resolves credentials in this order:

1. `GITHUB_TOKEN`
2. `GH_TOKEN`
3. `gh auth token`
4. `token` in `~/.perch/config.toml`

Using `gh auth login` is the simplest setup for the desktop app because a GUI
launch does not normally inherit shell environment variables. The credential
must be allowed to read and update notifications and to read the repositories
you open. Merging a pull request also requires `Contents: write` permission on
its repository. Commenting on, approving, or requesting changes on a pull
request requires `Pull requests: write` permission, as do resolving a thread
and reacting to a pull request or anything said on it; reacting on an issue
requires `Issues: write`.

## Configuration

Perch reads `~/.perch/config.toml` (or `$PERCH_HOME/config.toml`). A repository
can have a GitHub identity, local roots, or both:

```toml
poll_interval_secs = 60

[[repo]]
github = "slatedb/slatedb"
name = "slatedb"
roots = ["~/Code/slatedb"]
```

`name` is only the sidebar label. `roots` lets Perch discover existing local
branches and worktrees. Neither setting causes a repository-wide GitHub crawl.

Perch's database, avatars, managed clones, worktrees, and agent state live
under `~/.perch`. The database also holds local items, conversations, and
review state, so it is not disposable. GitHub notifications and fetched
remote data within it are refreshed on demand.

Schema upgrades use preserving, transactional migrations: local discussions,
agent attempts, session IDs, and activity history survive. Perch refuses an
unsupported or newer schema without deleting it, and a failed migration leaves
the original database in place.

## Agents

Questions about an item or selected lines in its Files view run through Claude
Code or Codex in that item's worktree. Follow-ups remain attached to the local
conversation, and the agents retain their read-only permissions. Nothing an
agent writes reaches GitHub unless you post it (see Posting).

### What the agent is told about the discussion

A question on a pull request or issue carries what was said on GitHub, as the
last fetch left it. The prompt quotes as much as fits in about 24,000
characters, in this order: each reviewer's latest verdict, the checks, the
threads within 20 lines of the lines asked about, the other unresolved threads,
the conversation tab, and the resolved threads. An entry goes in whole or not
at all; one comment is cut at 2,000 characters with a note of where the rest
is. Bots' comments stay out of the prompt. What does not fit is listed a line
each.

All of it, bots included, is also written as files to
`~/.perch/context/<run>/`: `INDEX.md`, `conversation.md` (the description,
comments, review summaries and timeline in time order), a file per review
thread under `threads/`, and `checks.md`. The prompt names the directory and
the agent reads what it needs. The directory is outside the worktree, belongs
to the run, and goes when the run is deleted; a retry reads the one its first
attempt had.

A follow-up that resumes the agent's session is told only what changed on
GitHub since the run it continues: new or edited replies in its thread, whole,
and new reviews, comments, threads and check results elsewhere. Nothing
changed, nothing is added. `perch --ask --print-prompt` prints the prompt a
question would get, and the files, without running an agent.

The app-wide activity inspector is one of the two things the drawer on the
right holds (see [The drawer](#the-drawer)). The persistent **Agents** control
in the status bar opens it, as do the Agents tile at the right end of the file
tabs and `Cmd-Shift-A`. Active work is grouped by worktree and continues while
another item is open or while the drawer is shut. Each row shows the agent,
model, effort, queue or execution time, and latest tool activity. Selecting
one shows its retained request, ordered activity, streamed response, usage,
and errors. Active rows can be stopped, queued rows can be cancelled, and
failed, stopped, or interrupted attempts can be retried without replacing
their history.

Perch executes at most four top-level agents by default and queues the rest in
FIFO order. Stopping a run terminates and reaps its process group. Closing
Perch does the same for every process it owns; after an abnormal exit,
nonterminal rows return as Interrupted and are not replayed automatically.

Perch discovers both CLIs automatically. Their optional settings are:

```toml
[agent]
binary = "claude"
model = "sonnet"
effort = "medium"              # low | medium | high | xhigh | max | ultra
models = ["fable", "opus", "sonnet", "haiku"]
max_concurrent_runs = 4

[agent.codex]
binary = "codex"
models = ["gpt-6-astra"]       # omit to use Codex's own model catalog
```

`perch --agents [limit]` prints active runs and retained history without
launching an agent. `perch --ask owner/repo#N path:start-end "question"`
runs the same question flow from a terminal; omit the path to ask about the
whole item, and add `--print-prompt` to see the prompt without running it.

### What the agent can and cannot do

An agent reads text nobody vetted: the pull request, its comments, and the
repository itself. Any of it can contain instructions aimed at the agent.
Perch does not try to detect them; it limits what following one could do.

- **Read-only tools.** Claude Code gets `Read`, `Glob`, `Grep`, `LS`, and
  `Bash` for `git diff`, `git log`, `git show`, and `git blame` only. `Edit`,
  `Write`, `MultiEdit`, `NotebookEdit`, `WebFetch`, and `WebSearch` are
  denied by name, so no setting can allow them. Anything else is refused
  without a prompt, and the refusal shows up as activity. Codex runs in its
  `read-only` sandbox.
- **A sandboxed shell.** Every Claude Code command runs inside Claude Code's
  own sandbox with no network access and no write access anywhere, the
  worktree included: `git diff --output=` fails like any other write. There
  is no unsandboxed retry, and a machine that cannot start the sandbox
  refuses to run rather than running without it.
- **No repository-supplied configuration.** Claude Code reads only
  `~/.claude/settings.json`, never the repository's `.claude/settings*.json`
  or `.mcp.json`, and starts no MCP server. Codex is told the worktree is
  untrusted, so its `.codex/` config, hooks, and rules are not loaded even
  when you have trusted the repository elsewhere, and it runs without
  `~/.codex/config.toml`, plugins, or hooks, since the MCP servers named
  there run outside its sandbox. Reviews run Claude Code in `--safe-mode`
  as well.
- **No secrets.** The agent does not inherit `GITHUB_TOKEN`, `GH_TOKEN`,
  Perch's own git token, or any variable named `*_TOKEN`, `*_SECRET`,
  `*_KEY`, `*_APIKEY`, `*_PASSWORD`, `*_PASSWD`, `*_PASS`, `*_PAT`, or
  `*_CREDENTIALS`. The CLIs' own credentials (`ANTHROPIC_API_KEY`,
  `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `OPENAI_API_KEY`,
  `CODEX_API_KEY`) are the exception, since without them there is no run.
  Credentials for other providers, such as `AWS_SECRET_ACCESS_KEY`, fall
  under the rule, so a CLI configured that way must be signed in instead.
- **Cleaned quotes.** Text quoted into a prompt keeps the words a reader
  sees and loses the rest: HTML tags and the comments GitHub renders as
  nothing, link titles and image alt text, and invisible formatting
  characters, however they were written. It is set between
  `[BEGIN untrusted text: …]` and `[END untrusted text]` lines, a line
  inside it that imitates a marker is spoiled, and your question comes
  last. The discussion files under `context/`, which the prompt says are
  untrusted like a marked block, and the review history export under
  `knowledge/reviews/` are cleaned the same way.
- **Inert links.** A link in an answer or a comment does nothing until it
  is clicked. Hovering it shows the full destination; a click opens `http`
  and `https` links in the browser and a file of the worktree in Files
  (see [Links to code](#links-to-code)), and nothing else.

What remains: the CLIs still read the repository's `CLAUDE.md` and
`AGENTS.md` as instructions, along with your own user-level configuration,
and the agent can read any file your user can, not only the worktree. What
it reads can reach you only through its answer.

## Worktree reviews

**Review** runs Claude Code and/or Codex's native review workflow against the
full committed branch diff from the selected base's merge base to `HEAD`. It
works for Perch-managed and registered external worktrees, with or without a
pull request. A dirty worktree is reviewable, but staged and unstaged edits are
not part of the snapshot; Perch labels this `Committed changes only; local
edits are excluded`. Staged-only, individual-commit, and incremental-only
review scopes are not supported. If the committed diff is empty, Perch records
that result without launching an agent.

The launcher's instructions box tells the reviewers what to focus on, such as
"review rfcs/multi_get.md only" on a branch that also carries a prototype. The
range reviewed stays the same: Claude reads the instructions after the range in
its `/code-review` argument, and Codex, which refuses a custom prompt beside
`--base`, reads them as developer instructions. A review keeps its
instructions, so **Retry** uses them again and its history card shows them.

A review of a pull request also reads its discussion. When the snapshot is
taken, Perch writes the discussion to `~/.perch/context/review-<id>/` in the
same shape a question gets it, and tells each reviewer, ahead of the requester's
instructions, where it is and what to do with it: not to report a problem a
reviewer already raised in an unresolved thread, and to report a change a
reviewer asked for that the range does not make, naming who asked. A review of
a branch with no pull request has no discussion to read.

Every review uses a clean, read-only, immutable snapshot in Perch-owned
storage. Perch retains its exact diff, manifest, commits, and anchored blobs so
history remains readable after the branch moves or an external checkout is
deleted. Opening a historical review never changes the current worktree.

Perch currently supports Codex CLI 0.155.1 or newer through `codex exec review`
and Claude Code 2.1.223 or newer through `/code-review`, when the installed CLI
advertises the required local, noninteractive options. Missing, older, or
incompatible CLIs are shown as unavailable with a reason; Perch does not fall
back to a generic review prompt. Review mode excludes Claude's hosted `ultra`
mode and never enables fix, comment, or permission-bypass options.

Findings are durable local discussions, separate from the attempts that found
them. Repeat reviews preserve every group, attempt, observation, failure, and
valid zero-finding result while reconciling the same problem back to its
existing thread. **Retry** creates another attempt with the original snapshot
and settings. **Review latest** creates a new group and immutable snapshot from
the branch's current committed state.

On an open pull request, a new finding on lines of its diff arrives as your
draft on those lines, in place of the reviewer's message. It is a draft like
one you write (see Posting): you edit or delete it, and it posts only when you
submit the review. The card above it still gives the severity and the
reviewer. Deleting the draft dismisses the finding, and dismissing the finding
or linking it as a duplicate deletes the draft. GitHub takes a review comment
only on a line its diff shows, a changed line or one of the three around it,
so this needs the review to be of what GitHub has: the pull request's head,
diffed from the same merge base. Any other finding stays in Perch as the
reviewer's message, as does every finding on a branch without an open pull
request.

Follow-ups stay local to Perch unless you post one yourself. Reviews do not
resolve threads on GitHub, modify code, run automatically after a push, or use
a hosted review service. Perch also exports each item's review history as
deterministic Markdown at
`$PERCH_HOME/knowledge/reviews/<owner>/<repo>/<item-id>.md` (under
`~/.perch` when `PERCH_HOME` is unset). Re-exporting replaces that stable
document rather than duplicating unchanged entries; unrelated legacy files in
the knowledge directory are not imported.

## The drawer

The drawer on the right holds one of two things: the open item's files as a
tree, or the Agents inspector. It is a column beside the main pane, so
opening it moves the pane over rather than covering what is being read, and
the open file, its scroll and a composer are kept through it opening,
shutting, and changing what it holds. When the window is too narrow for both
the drawer and a usable pane, the drawer lies over the pane's right edge
instead, from the top of the window to the status bar.

The two tiles at the right end of the file tabs open it, after the layout
tiles: Files and Agents, as Codex has its folder there. Each is a toggle. The
tile for what the drawer shows is filled, and a click on it shuts the drawer;
a click on the other swaps what it holds. `Cmd-Shift-E` and `Cmd-Shift-A` do
the same from anywhere, including from the code or a composer, and the Agents
control in the status bar is the third way to the inspector. `Escape` backs
out of a run's details and then shuts the inspector; the drawer's own close
button shuts it from either. What the drawer holds and how wide it is are
remembered between launches; asked for the files while an issue is open, it
stays shut until an item with files opens.

The files tree is the changed files until its filter is turned off, with the
scope button and the viewed count above it. A file opened from the tree opens
on Files, whichever tab the reader was on, and a file opened from anywhere
else, a card or a definition, is shown in the tree: the way to it opens, the
filter comes off when the file is not a change, and the tree scrolls to its
row. During a retained snapshot the tree has no scope to pick and nothing to
mark viewed, and the way back to the current files is in the tab strip.

## Going to a file

`Cmd-P` puts VS Code's Go to File over the top of the pane, from Files or
Conversation, for any item with files. Before anything is typed it lists
the files the item has had open, the most recent first, and then its
changes. As you type, it lists the recently opened files that match and
then every other file in the worktree that does, with the matched letters
in blue and a change's status letter at the right.

The ranking is VS Code's own. The letters must come in order. A run of
them beats the same letters spread out, a match in the name beats one
that needs the directory, and a name that starts with the query comes
first, shorter names before longer. A `/` in the query matches against
the whole path, words separated by spaces must all match, and a query in
double quotes must match as one run. A recently opened file is listed as
recent only when the letters form one run in its name, or in its path for
a query with a `/`; otherwise it is ranked with the other files.

`Enter` opens the file on Files, and `config.rs:42` opens it at line 42;
`:42` alone goes to line 42 of the open file. The list of recently opened
files is kept for each item until Perch quits.

| Action | Key |
|---|---|
| Go to File | `Cmd-P` |
| Next, previous | `Down`, `Up`, or `Ctrl-N`, `Ctrl-P`; `Cmd-P` again for next |
| Open | `Enter` |
| Close | `Escape`, or a click outside it |

## Code navigation

Files reads like an IDE when rust-analyzer is installed. Rest the pointer on
an identifier and a tooltip says what it is: the signature in the code font,
then the first lines of its doc comment. The tooltip cannot be entered and has
no scroll. It opens where the pointer is and goes away when the pointer does,
so nothing has to be dismissed; the panel under the editor is where a list
goes, and the tooltip is a glance. A reader who wants the whole doc comment
goes to the definition.

A definition in a dependency or the standard library opens as a tab like any
other, with a chip naming where it is from (`tokio 1.40.0`, `std`, `git:
floem`). Such a tab is a place to read: whole, with no diff, no Viewed and no
threads, and it is not remembered when the item is closed.

The keys are VS Code's. Right-click an identifier for the same entries, each
naming the symbol it acts on; on whitespace the menu is Copy alone.

| Action | Key |
|---|---|
| Go to Definition | `Cmd-click`, `F12` |
| Go to Type Definition | menu |
| Go to Implementations | `Cmd-F12` |
| Find All References | `Shift-F12` |
| Show Call Hierarchy | `Shift-Alt-H` |
| Back, Forward | `Ctrl--`, `Ctrl-Shift--` |
| Close the panel | `Escape` |

One location opens; several open the panel and the reader picks. The call
hierarchy is a tree: a chevron on a row asks for that row's callers, and a
toggle in the panel's header turns it to callees. The caret resting on an
identifier tints its other uses in the file. All of it is read-only, and all
of it is rust-analyzer: while the server is absent or failed, the status line
says so and nothing else happens.

`Cmd-F` puts a find bar over the top right of the code, as in VS Code: a box
for the text, toggles for match case, whole word and regular expression, the
count, and arrows. Every match in the open file is tinted, the current one
darker and selected, and the editor scrolls to it; as the text is typed the
bar moves to the nearest match from the caret, and a match inside a folded
run reveals the run. `Cmd-F` with something selected on one line looks for
that. The bar stays up from one file to the next, looking for the same text
in each, and searches a diff's removed lines along with the rest in the
stacked layout.

| Action | Key |
|---|---|
| Find | `Cmd-F` |
| Next match, previous match | `Enter`, `Shift-Enter`, or `Cmd-G`, `Cmd-Shift-G` |
| Match case, whole word, regular expression | `Alt-Cmd-C`, `Alt-Cmd-W`, `Alt-Cmd-R` |
| Close the bar | `Escape` |

An SVG opens as the picture it draws rather than as its markup, fitted to the
pane. A changed one shows before and after side by side, both at one scale,
so a change of size shows as one; an added or deleted one says which it is.
The status line gives the picture's size. The Source tile at the right end
of the tab strip shows that file's markup instead, with its diff, gutter and
threads, and the picture is back at the next click.

## Links to code

A file, a line or a symbol of the worktree that a comment or an answer
names is a link to it, on Conversation and in the cards over the code: in
a code span, in a fenced block, or as a link's destination. A click goes to
Files, opens the file with the caret on the line, and `Ctrl--` comes back.
Hovering says where it goes. It is drawn as any link is, so the text reads
as it did.

A file links when the worktree has it. A line follows the path as the
compiler writes one, `src/db.rs:17` or `src/db.rs:17:5`, or as GitHub
does, `src/db.rs#L17`; a range opens at its start, and the line is the
head's, which is what an agent reads. A path can be written short,
`wal/mod.rs:303` or `gc.rs`, and links when only one file ends with it, or
only one of the changed files does. An absolute path links only inside the
worktree: nothing outside it opens from a comment.

A symbol links through rust-analyzer's workspace symbol search: a name,
`WalGcPolicy` or `list_wal_ssts`, or a name after what it belongs to,
`Db::open`, `WalGcMode::Regular`. One definition opens and several open the
panel as its definitions, generated code left out when there is code
written by hand. Fields are not in the search, so `DbOptions.ttl` links
`DbOptions`. A lone word in lower case, such as `inner`, is looked up only
when it is called, `inner()`, since it is as often a field or a variable;
and a name with more than ten definitions, such as `new`, links nothing.
In a fenced block the paths link whatever the language, and the names that
read as types do when the block is Rust, a diff, output or of no language
named. A name links
once rust-analyzer has answered, a moment after the text is drawn; while
the server is absent or failed, only files do.

## Development

```sh
cargo test --workspace
cargo run -p perch-app
```
