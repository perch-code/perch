# Perch: Milestone 17, New worktrees and Work runs

## Goal

Let the reader start work in Perch, not only read it. Today a worktree
comes from somewhere else: a pull request Perch checks out, a branch
the root scan finds in one of the reader's clones, or Start Work on an
issue, which always cuts `perch/issue-N` from the default branch of
Perch's bare clone. There is no way to say "a new branch of this
repository, off that branch, called this". A repository that lives only
on this machine can't get a worktree from Perch at all. Start Work and
Fork need a bare clone, so they refuse with "no home checkout to cut
from". And every agent in Perch is read-only, so once a worktree
exists, the reader does the work in another tool.

After this milestone a `+` at the top of the sidebar opens a composer
for new work. It has three chips under it: the repository, the branch
to start from, and the name of the new branch. Sending it makes the
worktree, opens it as a branch item, and starts an agent on the message
in that worktree. The agent can edit files and run commands there. It
works in a sandbox: it can write only to the worktree, it has no
network beyond what the repository's config allows, and it has no
credentials. It does not commit or push. The reader does, from a
terminal, and Perch shows what each run changed.

This works the same for a GitHub repository and for one that has never
had a remote. When a GitHub branch is pushed and a pull request opens
for it, the row becomes the pull request's, as a branch row does
today.

The milestone turns on two decisions.

The first is that Perch has no projects and does not grow them. A
`[[repo]]` entry already is one: a name, a GitHub identity or `local/`,
and the clones on disk. The new-work composer lists those entries, and
the result is an item like every other. The sidebar stays one list.

The second is that write access follows whose work it is, not a setting.
The read-only agent exists because a pull request's text and code come
from people the reader has not vetted. That is still true of someone
else's pull request, and it stays Ask-only. A branch the reader made or
checked out, or a pull request the reader authored, is the reader's own
work. There the agent may write inside a sandbox built for it. The
reader picks Ask or Work on each message, and Work is offered only
where it is allowed.

## Non-goals

- Approvals. A Work run cannot ask to do something outside its sandbox.
  What the sandbox refuses is refused, and the refusal shows up as
  activity, as it does today. Host-answered permission prompts are
  Milestone 18 (see the end of this document).
- Agent commits. The worktree's git directory stays read-only to the
  agent, so `git commit` fails like any other write outside the
  worktree. Committing needs write access to the repository's git
  directory without its `config` and `hooks`. That is an approval-shaped
  grant, and it isn't clear Codex's sandbox can express it. Milestone 18.
- Pushing, opening a pull request, or any GitHub write from a Work run.
  The agent has no token and no route to GitHub.
- A commit or push action in Perch. The reader commits in a terminal.
  **Open in Terminal** is already on the row and in the summary.
- Loading the repository's own agent configuration: `.claude/settings*`,
  `.mcp.json`, `.codex/`, hooks, MCP servers. Work runs keep Ask's
  isolation. A per-repository trust switch is Milestone 18.
- Work on someone else's pull request, on an issue, or in a review
  snapshot.
- Projects, repository groups, or sections in the sidebar.
- A settings UI. **Add repository…** appends a `[[repo]]` table, and
  everything else is edited in `config.toml`.
- Renaming a branch after it is made. Git can, and a pushed branch
  makes it messy. The composer is where the name is chosen.
- Running two Work runs in one worktree at once.
- An embedded terminal, an editor that writes, or a dev server.

## Behaviour

### New work

A `+` at the right of the sidebar's scope tabs, and `Cmd-N` from
anywhere, open the new-work view in the main pane. The view is a
composer the width of the conversation column, with the destination
picker Milestone 15 gave every composer (`Claude · Opus · High`, `Codex
· …`, never GitHub) and three chips under the box:

    [▢ perch]  [⑂ main]  [☑ New branch  chris/fix-the-thing]

The **repository** chip opens a searchable list of the `[[repo]]`
entries, most recently used first. Each row gives the display name, and
a GitHub entry its `owner/repo`. A local one is marked `local`. The
list ends in **Add repository…** (see below). It opens on the repository
of the item that was open, or else the last one used.

The **base** chip opens a searchable list of branches (see Where the
worktree comes from for what each kind of repository lists). It opens
on the repository's base branch as the root scan finds it: the remote's
HEAD, else `base`, else `main` or `master`.

The **branch** chip holds a checkbox and a name. With **New branch**
ticked, Perch cuts a new branch off the base under that name. The name
fills in as the message is typed. It is `branch_prefix` from `[work]`,
then a slug of the message's first five words (lowercase ASCII,
hyphens), or `work-<n>` for an empty message. A name the reader edits
stays as typed. A typed name that is taken is an error under the chip.
A name Perch filled in gets `-2`, `-3` instead, as `cut_branch` does
today.

With **New branch** unticked, the name field goes and Perch checks out
the base branch itself: the local branch if there is one, else a new
local branch tracking the remote one. Git won't check out one branch in
two worktrees. If the branch is already checked out, whether in the
root, in a worktree the scan found, or in one Perch made, Perch opens
that item instead and says so.

Send reads **Start** with a message and **Create worktree** without one.
Sending shows the progress line an opening pull request shows today
("Fetching main", "Adding the worktree"). Then it opens the new branch
item on Conversation, with the message as the first Work run, already
queued. The item's name is the message's first line, as Rename would
set it, or the branch when there is no message. `Escape` or a click on
another row leaves the view and keeps the text for the next `Cmd-N`
until Perch quits.

### Add repository…

This opens the system folder chooser. The folder must be inside a git
repository. Perch takes the repository's top level (for a linked
worktree, its main worktree) and refuses a folder that already is a
root of an entry. Then Perch appends to `config.toml`:

```toml

[[repo]]
roots = ["~/Code/notes"]
```

It appends text and does not rewrite the file, so the reader's comments
and order survive. A path under the home directory is written with
`~`. Perch then reloads the `[[repo]]` entries, resolves the new one the
way startup does (a GitHub remote makes it that repository, otherwise
it is `local/<name>`), scans its root, and selects it in the chip. When
`config.toml` does not exist, Perch creates it with just this table.

### Where the worktree comes from

Worktrees live where Perch's always have, `~/.perch/worktrees/`, named
`<owner>__<repo>__<branch>` with `local` as the owner of a local
repository. What changes is the repository they are added from.

- **An entry with a root**, GitHub or local: the root. Perch runs `git
  worktree add` in the reader's own clone. The new branch lives there
  with the reader's remotes, config and other branches, and `git
  worktree list` in the clone shows it. The base list is the clone's
  local branches, then its remote-tracking branches for the remote
  `github_remote` picks (or `origin`), each once, the base branch
  first. For a remote-tracking base, Perch first fetches that one
  branch from that remote in the root, under the clone's own
  credentials. If the fetch fails, the cut still happens from what the
  clone has, and a notice says so and how old the ref is.
- **A GitHub entry with no root**: Perch's bare clone, as Start Work
  uses today. The base list is `git ls-remote --heads origin`, cached
  for the session, the default branch first. Perch fetches the chosen
  base into `refs/perch/home/<base>` and cuts from there, which is what
  `home_ref` already resolves.

Perch now writes into a root, where before it only read one. It writes
exactly this much: the new branch ref, git's record of the linked
worktree under `.git/worktrees/`, a fetch of one branch, and, on
Delete, removing that branch (see Keeping work safe). It never runs
anything else in a root, never resets or checks out in it, and never
fetches all remotes.

Every git command Perch runs in a root is non-interactive:
`GIT_TERMINAL_PROMPT=0` and `GIT_SSH_COMMAND` with `-o BatchMode=yes`.
Perch's GitHub token is not added. A passphrase-locked key or an
unreachable remote makes the fetch fail rather than hang.

The item is an owned branch item. `owned` still means Perch made the
directory and may remove it. The checkout also records the repository
it was added from, so removing it prunes in the right place. The root
scan runs `git worktree list` in the root and so will see these
worktrees. It already skips a branch whose item is owned, and it goes
on doing so.

A local repository's Start Work and Fork use its root now instead of
refusing. Start Work on an issue is unchanged otherwise, and the item
it makes is Work-eligible.

### Ask and Work

A composer on an eligible item gains a two-way switch before the
destination picker: **Ask** and **Work**. An item is eligible when:

- it is a branch item (no pull request or issue), owned or found by
  the root scan, with its directory on disk; or
- it is a pull request whose author is the reader, as the Authored
  scope decides, with a checkout on disk.

Nothing else is eligible: not someone else's pull request, not an
issue, not a finding's snapshot, not a thread asked about a line of a
retained snapshot. On those the switch is absent and every message is
Ask, exactly as today.

The switch is remembered per item. It opens on Work on an item the
new-work view made and on Ask everywhere else. A follow-up in a thread
follows the thread's last run, so a Work thread continues as Work
unless it is flipped. The run card says `Work` or `Ask` beside the
model, and the Agents inspector says the same on the row.

Ask is Milestone 16's run, unchanged. Work is a different permission
set on the same machinery: the same queue, prompt building, discussion
context on the reader's own pull request, session resume, and stop and
retry.

### The Work sandbox

A Work run's working directory is the worktree. What it can do:

- **Edit files.** Claude Code gets `Edit`, `Write` and `MultiEdit`,
  scoped by permission rules to paths under the worktree, and
  `NotebookEdit` the same way. Codex runs in `workspace-write`, whose
  writable root is the worktree.
- **Run commands.** Claude Code gets `Bash` without a command list. The
  sandbox is now the boundary: `autoAllowBashIfSandboxed: true`,
  `allowUnsandboxedCommands: false`, `failIfUnavailable: true`. Codex
  runs any command inside its sandbox as it does in `workspace-write`.
- **Write** the worktree, the system temporary directory, and the paths
  in the repository's `[repo.work] write` list, and nothing else. Both
  sandboxes deny writes to `<worktree>/.git` (the file that points at
  the git directory), `.claude/`, `.codex/` and `.mcp.json` in the
  worktree. The git directory itself is outside the worktree and not
  in any list, so it is read-only.
- **Reach the network** only for the domains in `[repo.work] network`.
  The default list is empty. Claude Code takes the list as
  `allowedDomains` with `strictAllowlist`. Codex's stable sandbox has
  network or doesn't, with no allowlist, so a Codex Work run has no
  network whatever the list says, and the README says so. Codex 0.156
  has an experimental `network_proxy` feature. Perch doesn't turn it
  on until it is stable.

What doesn't change from Ask: `WebFetch` and `WebSearch` stay denied by
name. `--setting-sources user`, `--strict-mcp-config` with no servers,
and Codex's untrusted-project override and `--ignore-user-config` stay.
Every `*_TOKEN`-style variable is scrubbed as today. No unsandboxed
retry. A machine that can't start the sandbox refuses the run and says
why, and never falls back to Ask or to no sandbox.

The per-repository settings:

```toml
[work]
branch_prefix = "chris/"

[[repo]]
roots = ["~/Code/perch"]

[repo.work]
write = ["~/.cargo/registry", "~/.cargo/git", "~/.cargo/.package-cache"]
network = ["index.crates.io", "static.crates.io"]
```

A path in `write` is a place the agent can leave something for the next
program that reads it. The README's example names cargo's cache
directories and not `~/.cargo`, because `~/.cargo/config.toml` and
`~/.cargo/bin` would let a run change what `cargo` does when the reader
runs it outside the sandbox. Perch refuses a `write` entry that is, or
contains, the home directory, a root's `.git` directory, or
`~/.perch`, and names it in a notice at startup.

### The Work prompt

A Work run's system text says what Ask's says about untrusted text,
and adds:

- the repository, the branch, the base, and that it is working in a git
  worktree;
- that it may edit files and run commands in the worktree, and that
  writes elsewhere and network access outside the listed domains will
  be refused;
- that it must not commit, push, or change branches: the git directory
  is read-only, and the reader commits;
- to end with what it changed and what it ran to check it.

On the reader's own pull request the discussion section and directory
from Milestone 16 come with it, as for Ask.

### One Work run per worktree

At most one Work run is active or queued-to-start in a worktree at a
time. A second one sent while one is running is admitted and waits in
the queue with `Waiting for the Work run in this worktree`, even when
there are free slots. It starts when the first finishes, fails or is
stopped. Ask runs and reviews are not held back. A review reads a
committed snapshot, and a Work run doesn't commit.

### What a run changed

Before a Work run starts and after it ends, however it ends, Perch
records the worktree as a commit. It builds a temporary index from
`HEAD`, adds every tracked and untracked file (not ignored ones), runs
`write-tree`, then `commit-tree` with `HEAD` as parent. The commits are
kept under `refs/perch/runs/<run id>/before` and `/after` in the
repository the worktree was added from, so `gc` keeps them. Perch runs
this with its own git, outside the sandbox, and it doesn't touch the
worktree, its index, or `HEAD`.

The run card shows `4 files changed, +120 −30` when the two differ.
Clicking it opens Files on a new scope, **Run**, which diffs `before`
against `after` for that run. The scope picker lists it for the run
the card belongs to, as it lists a commit. A run that changed nothing
shows nothing. Deleting the run, its thread or its item deletes the
refs.

The Uncommitted scope keeps showing everything not yet committed, from
every run and from the reader's own edits.

### Keeping work safe

A worktree made for work can hold the only copy of it. Three things
Perch does today assume the work can be fetched again, and each
changes. The first two apply to every owned item, not only new ones.

- **Delete.** Before `branch -D`, Perch counts the branch's commits
  that no remote-tracking branch and no base holds: `git rev-list
  --count <branch> --not --remotes <base>`, and for a local repository
  `--not <base>`. A nonzero count turns Delete's confirmation into
  `chris/fix-the-thing has 3 commits that are on no remote. Delete
  them?`. A dirty worktree still refuses, as today.
- **A pull request's worktree when its head moves.** `prepare` resets
  a clean worktree to the new head. That now happens only when the
  worktree's `HEAD` is an ancestor of the new head (`merge-base
  --is-ancestor`). Otherwise the worktree is left where it is and
  reported stale, as a dirty one is. A Work run doesn't commit, but
  the reader does, and a clean worktree with an unpushed commit would
  lose it.
- **Archive.** Archive removes the directory. `git status --porcelain`
  doesn't see ignored files, so a `.env` or a notes file goes with it.
  Archive's confirmation on a branch item now names the top-level
  ignored entries it will delete (`status --porcelain --ignored`,
  first ten, then `and N more`). The 14-day sweep no longer archives
  branch items. It goes on archiving pull request checkouts.

## Model

In `perch_github::items`:

- `Checkout` gains `source: Option<PathBuf>`: the repository the
  worktree was added from, when it is a root. `None` is Perch's bare
  clone, as for every owned checkout today. `remove_worktree` and
  `delete_branch` take the repository from it.
- `items` gains `source_path TEXT`, one migration.

In `perch_github::config`:

- `RepoConfig` gains `work: WorkConfig { write: Vec<String>, network:
  Vec<String> }`, both defaulting to empty. Paths expand `~`.
- `Config` gains `work: WorkDefaults { branch_prefix: String }`.
- `append_repo(root) -> Result<()>`, which writes the table as above.

In `perch_agent`:

- `Mode { Ask, Work(WorkGrant) }` on `RunSpec`, replacing the bare
  `allowed_tools` field. `WorkGrant { write: Vec<PathBuf>, network:
  Vec<String> }`. `READ_ONLY_TOOLS` stays Ask's list, and reviews keep
  using it directly.
- `claude_settings(cwd, mode)`: Ask is today's JSON. Work sets
  `autoAllowBashIfSandboxed`, `filesystem.allowWrite` (worktree, temp,
  grant), `filesystem.denyWrite` (the four worktree paths above),
  `network.allowedDomains` from the grant, and `permissions.allow`
  with `Edit`, `Write`, `MultiEdit` and `NotebookEdit` rules on the
  worktree in Claude Code's absolute-path form.
- `codex_args` for Work: `-s workspace-write`, `--add-dir` for each
  grant path, `-c sandbox_workspace_write.network_access=false`. On
  resume, which rejects `-s`, the sandbox mode and
  `sandbox_workspace_write.writable_roots` go in as config, as Ask
  repeats `sandbox_mode` today. Check by hand whether `exec resume`
  takes `--add-dir`.

In `perch_github::cache`, `runs` gains `mode TEXT NOT NULL DEFAULT
'ask'`, one migration with the item's.

In `perch_git`:

- `cut_branch` takes the repository to add from instead of a `RepoId`,
  so a root and the bare clone go through one function.
- `branches(repo, remote) -> Vec<BaseBranch>` for the base list, and
  `fetch_branch(repo, remote, branch)` with the non-interactive
  environment.
- `unpushed_count(repo, branch, base)`, `ignored_entries(worktree)`,
  `is_ancestor(repo, a, b)`.
- `snapshot_worktree(repo, worktree, ref)` for a run's before and after.
- `Scope::Run { before, after }`.

## Backend

- `Command::CreateWorktree { repo, base, branch: Option<String>,
  message: Option<ComposerMessage> }`. `None` for `branch` checks out
  the base. It resolves the source, fetches, cuts, inserts the item,
  opens it, and then, with a message, sends the ask as a Work run on
  the new item. The steps report through `ReviewProgress`.
- `Command::AddRepository { path }`, which appends, reloads the repo
  entries in `Ctx`, and scans the new root. The loaded `Config` becomes
  swappable for its `[[repo]]` list only. Nothing else reloads.
- `no_home_for_local` is replaced by a source lookup: a root when the
  entry has one, else the bare clone, else the existing message.
- `ask` takes the mode. It checks eligibility itself and refuses Work
  on an ineligible item. The UI's switch is not the guard.
- `runs::admit` gains the per-worktree Work hold. The hold is keyed on
  the checkout's resolved path, so two items can't share it through
  a symlink.
- The run's before snapshot is taken as the run leaves the queue, not
  at admission, so a run queued behind another sees the first run's
  result as "before". The after snapshot is taken on every terminal
  state.
- `delete_item` removes a run's refs with the run.

## Harness

Pure tests:

- `claude_settings` and `RunSpec::args` goldens for Ask (unchanged)
  and Work with an empty grant and a full one, both backends, new
  thread and resume. Work never carries `bypassPermissions`,
  `allowUnsandboxedCommands: true`, `--dangerously-*`, or
  `danger-full-access`.
- Eligibility over the item shapes: own branch, scanned branch, own
  pull request, someone else's, issue, archived, off disk.
- Branch naming: the slug, the prefix, `work-<n>`, a taken filled-in
  name getting `-2`, a taken typed name refused.
- `append_repo` on a missing file, an empty one, and one with comments
  and top-level keys. The result parses, and the old text is
  byte-for-byte the new text's prefix.
- `write` entries refused: the home directory, a parent of it, a root's
  `.git`, `~/.perch`.

Temporary-repository tests in `perch_git`:

- Cut from a root: the branch is in the root, the worktree is under the
  store, and `git worktree list` in the root lists it. The same for a
  repository with no remote.
- Check out an existing local branch, a remote-only one, and one
  already checked out (refused, with the path).
- A fetch against a remote that asks for a password fails at once.
- `unpushed_count` with none, with commits, and on a local repository.
- The ancestor rule: a clean worktree with a local commit is left alone
  when the head moves, and one behind is moved.
- `snapshot_worktree` before and after an edit, an added untracked file
  and a deletion. The worktree's index and `HEAD` are the same after
  as before.

By hand, against the real CLIs, like the review tests. Run each case
once in Claude Code and once in Codex:

- a Work run asked to edit a file succeeds;
- asked to `echo x > ~/perch-escape` fails;
- asked to `git commit -am x` fails;
- asked to `curl https://example.com` fails;
- asked to write `.claude/settings.json` in the worktree fails.

Record the refusal each backend gives in the README.

## Build order

1. Keeping work safe: the ancestor rule in `prepare`, Delete's count,
   Archive's list, the sweep leaving branch items. This protects
   what exists today before anything writes. Commit.
2. `Checkout.source`, the migration, `cut_branch` over a repository,
   local repositories using their root for Start Work and Fork. Commit.
3. `perch_git` branch listing, non-interactive fetch, check out
   existing. `Command::CreateWorktree` without a message. Commit.
4. The new-work view: `+`, `Cmd-N`, the three chips, **Add
   repository…** and the config append. Commit.
5. `perch_agent`: `Mode`, the Work settings and args for both
   backends, goldens, the by-hand CLI checks. Commit.
6. The app: the Ask/Work switch, eligibility, the Work prompt, the
   per-worktree hold, `runs.mode`, the first message from the new-work
   view. Commit.
7. What a run changed: snapshots, refs, the card line, the Run scope.
   Commit.
8. README: a section on starting work; under "What the agent can and
   cannot do", Work beside Ask, what its sandbox allows and what it
   doesn't, the Codex network difference, and what a `write` entry
   risks; `[work]` and `[repo.work]` under Configuration; Delete,
   Archive and the pull request reset under Worktrees. Commit.

## Acceptance

Against a fresh `git init ~/tmp/notes` with one commit and no remote,
this repository as a root with a GitHub remote, and a GitHub repository
with no root:

- **Add repository…** on `~/tmp/notes`. `config.toml` ends in the new
  table and the rest of the file is as it was. `notes` is in the
  repository chip, marked `local`.
- `Cmd-N`, `notes`, base `main`, message "Add a README with a one-line
  description". The item opens on Conversation with a Work run
  running. Then:
  - The worktree is `~/.perch/worktrees/local__notes__…`.
  - `git -C ~/tmp/notes worktree list` shows it on the new branch.
  - When the run ends, the card says `1 file changed` and opens the Run
    scope with `README.md` added.
  - `git -C ~/tmp/notes status` is as it was.
- In the same thread, ask Work to commit. The run reports that git
  refused, and the branch has no new commit. Commit from **Open in
  Terminal**. Delete the item: the confirmation says one commit is on
  no remote. Cancel.
- This repository: base `origin/<some other branch>`, a typed name,
  message "Add a test for `slugify`". Then:
  - The fetch runs.
  - With `[repo.work]` giving cargo's caches and crates.io, the run's
    `cargo test` builds and runs.
  - Without them, it fails on the lock file or the network, and the
    activity says which.
  - Commit and push from a terminal, and open a pull request on
    github.com. On the next refresh the row is the pull request's, the
    switch still offers Work, and its runs carry the discussion.
- Untick **New branch** and pick the branch the last step made. Perch
  opens that item and doesn't make a second worktree.
- The GitHub repository with no root: base `main`. The worktree comes
  from the bare clone, and `refs/perch/home/main` is fetched.
- Open someone else's pull request. The composer has no switch. `perch
  --ask` on it runs read-only, as today.
- Send two Work messages in one worktree. The second waits with the
  hold's text while an Ask on another item runs beside the first.
- A pull request of the reader's in a `perch/pr-N` worktree: commit
  locally without pushing, then push a new head from elsewhere and
  refresh. The worktree is reported stale and the local commit is
  still there.
- A Codex Work run edits a file, and its `curl` is refused even with
  `network` set.
- Archive a branch item with a `.env` in its worktree. The confirmation
  names `.env`.

## Things the implementer should not do

- Do not offer Work, or run it, on anything but an eligible item. The
  backend checks. The switch is not the guard.
- Do not give a Work run the git directory, the root's `.git`, or any
  path outside the worktree, temp and the configured `write` list.
- Do not pass `bypassPermissions`, `--dangerously-bypass-approvals-and-sandbox`,
  `danger-full-access`, or `allowUnsandboxedCommands`. Do not retry a
  refused run without the sandbox, and do not fall back to Ask
  silently.
- Do not give the agent a token or a remote it can push to through
  Perch.
- Do not run anything in a root beyond the worktree add, the one-branch
  fetch, and deleting the item's own branch. No reset, checkout,
  `fetch --all`, stash or gc there.
- Do not let a git command in a root prompt. No terminal prompt, no
  SSH passphrase prompt, and no Perch token added.
- Do not `git worktree remove --force`, and do not `branch -D` without
  the unpushed count.
- Do not rewrite `config.toml`. Append.
- Do not load the repository's `.claude/`, `.mcp.json` or `.codex/` for
  a Work run.
- Do not add a project concept, a grouped sidebar, or a second list.

## Follow-up: Milestone 18, Approvals

Recorded here so it isn't lost. It is not part of this milestone.

A Work run that hits its sandbox's edge today just fails the step. In
Milestone 18 it asks, and the reader answers in the run's card:
**Allow once**, **Always for this repository**, **Deny**. The request
names the tool and its input (the command, the path, the domain) in
the card, cleaned like any agent text.

- **Claude Code.** Runs switch to `--input-format stream-json` and
  `--permission-prompts host`, so a permission request arrives on the
  run's output and Perch writes the answer to its input. Perch becomes
  a long-lived peer of the process rather than a reader of its output.
  That changes how a run's stdin is held and how stop and retry work.
  A run waiting on an answer holds its slot and shows `Waiting for
  you` in the inspector and on the row. It times out to Deny after a
  fixed interval.
- **Codex.** `codex exec` cannot ask. Approvals need `codex app-server`
  and its JSON-RPC protocol, which is what Codex Desktop drives. That
  is a second transport for Codex runs, and it is most of the work.
- **Grants.** "Always for this repository" is stored in Perch's
  database per repository identity, not in `config.toml`, as a tool
  and a pattern (`Bash(cargo publish:*)`, a domain, a path). Grants
  are listed and revocable. A grant is added to the Work settings of
  later runs, so they don't ask again.
- **Commits.** Committing is the first grant worth offering. It needs
  write access to the objects, refs and logs of the repository the
  worktree came from and to its `worktrees/<name>/` entry, but not
  `config`, `config.worktree` or `hooks`, where one write becomes code
  the reader later runs outside any sandbox. Claude Code can express
  that with `allowWrite` and `denyWrite`. Codex's writable roots have
  no documented way to exclude a subpath. Check again then; if there
  still isn't one, Codex commits go through app-server approvals.
- **Repository trust.** A per-repository `trust = true` that loads the
  repository's `.claude/`, `.mcp.json` and `.codex/` for Work runs. MCP
  servers and hooks run outside the sandbox. That is why it is opt-in,
  per repository, and never a default.
