# Perch: Milestone 18, Approvals

## Goal

Let a Work run ask. Today a Work run that reaches the edge of its
sandbox is refused. A `curl` to a host not in `[repo.work] network`
fails, a write outside the worktree fails, and nothing reaches the
reader but a failed step in the activity. The reader's only remedy is
to edit `config.toml` and send the message again.

After this milestone the request comes to the reader instead. The run's
card shows what the agent wants to do: connect to `example.com`, run
`git commit -m …` outside the sandbox, write a file outside the
worktree. It has three buttons: **Allow once**, **Always for this
repository**, and **Deny**. The run waits for the answer, which is then
passed back to the agent. "Always" is kept as a grant on the
repository, and later requests it covers are answered at once, without
asking. The item's summary lists the repository's grants, and any of
them can be taken back.

This works for both agents. Claude Code is driven through its host
permission protocol: `--input-format stream-json` with
`--permission-prompt-tool stdio`. Codex is driven through
`codex app-server` and its JSON-RPC protocol, the protocol Codex's own
desktop app uses, which is the only way Codex asks a host anything.

Two grants of M17's follow-up come with it. **Commits**: a grant that
lets Claude Code Work runs in a repository commit, with the repository's
git directory writable except its `config` and `hooks`. **Trust**: a
`[repo.work] trust = true` that loads the repository's own agent
configuration for Work runs.

The milestone turns on one decision: Perch answers from its own grants,
and never writes the CLIs' permission stores. Claude Code offers to add
a rule to `.claude/settings.local.json`, and Codex offers to amend its
exec policy under `~/.codex`. Perch uses neither. A grant lives in
Perch's database, keyed by repository and by what it allows, and Perch
answers a request the grant covers the moment it arrives. So a grant
means the same thing for both agents. It is listed where the reader
can see it, and revoking it cannot leave a rule behind in a file the
reader never opened.

## Non-goals

- Approvals for Ask runs. A question stays read-only in print mode.
  Nothing can prompt, as today.
- Approvals for reviews. They run as in M13, read-only.
- `WebFetch` and `WebSearch`. They stay denied by name. A Work run
  reaches the network through the sandbox, where a request names a host
  the reader can judge.
- Codex's automatic approval reviewer (the "guardian"). Perch asks the
  reader, and sets `approvalsReviewer` to `user` whatever the user's
  config says.
- Grants for Codex that change Codex. `acceptWithExecpolicyAmendment`
  is never sent, and nothing is written under `~/.codex`.
- Pushing. A grant to run `git push` outside the sandbox is possible,
  since any command can be allowed, but Perch does not suggest it and
  gives the agent no token.
- A settings screen for grants. They are listed on the item and removed
  there. `config.toml` stays hand-edited.
- Remembering a pending request across a restart. A run waiting for an
  answer when Perch quits is interrupted, as any running run is.

## Behaviour

### What asks

A Work run asks about three kinds of thing.

- **A host.** A command in the sandbox connects to a host not in
  `[repo.work] network`. Claude Code sends `SandboxNetworkAccess` with
  the host. Codex asks to run the command with network access, naming
  the host when it knows it, and otherwise the command. The card reads
  `Connect to example.com`. (Checked against Codex 0.156: a `curl` came
  as a command, with the host only in its reason, so its "Always" keeps
  the command.)
- **A command outside the sandbox.** The agent retries a command the
  sandbox refused, asking to run it without the sandbox. Claude Code
  sends `Bash` with `dangerouslyDisableSandbox`. Codex sends a
  `commandExecution` approval with its reason. The card reads `Run
  outside the sandbox` and shows the command in the code font, with
  the agent's reason under it when it gives one.
- **A tool on something outside the worktree.** Claude Code asks for
  an `Edit` or `Write` outside the worktree, or for a tool that is not
  on its list. Codex asks for a `fileChange` outside its writable
  roots, or for more permissions. The card reads `Edit
  /Users/me/.zshrc`, or `Use <tool>` with the CLI's own description.

Everything else is decided as in M17 without asking. Commands inside the
sandbox run. Edits in the worktree are made. The protected paths
(`.git`, `.claude`, `.codex`, `.mcp.json`), `WebFetch` and `WebSearch`
are refused by name before any request is made.

### The card

A pending request appears in the run's block, under its steps, and
above anything else the run has said. It shows:

    ⚠ Run outside the sandbox
    git commit -m "Add a README"
    The sandbox blocked writing .git/index.lock.

    [Allow once]  [Always let Work commit here]  [Deny]

The buttons are Allow once, the Always button, and Deny. Allow once
answers yes to this request only. Deny answers no, with the message
"The reader said no", which the agent sees as the tool's error. The
Always button:

- for a host: **Always allow example.com here**. The grant
  `network:example.com`.
- for a command outside the sandbox that is a git command that writes
  the repository (`add`, `commit`, `stash`, `rm`, `mv`, `restore`,
  `reset`, `checkout`, `switch`, `merge`, `rebase`, `cherry-pick`,
  `revert`, `tag`): **Always let Work commit here**. The grant
  `commit`, see Commits.
- for any other command: **Always allow this command here**. The grant
  `command:<the command exactly>`. A command that differs by one
  character asks again.
- for a tool: **Always allow this here**. The grant `tool:<tool>:<what>`,
  where `<what>` is the path for an edit or the CLI's own rule content
  when it suggests one (`domain:example.com`).

The request stays on the card while the run waits, and the card goes
as soon as it is answered. The run's steps keep both halves: `Asked:
Connect to example.com` when it is asked, then `Allowed once: …`,
`Always allowed: …`, `Denied: …` or `Allowed by a grant: …`. So they
survive a restart and show in the Agents inspector. (As built: a step's
own words cannot change after it is written, so the question and the
answer are two steps rather than one line that changes.)

While a run waits, the Agents inspector's row and the run's activity
say `Asked: Run outside the sandbox: <command>`. The run keeps its slot
and its worktree lock. After fifteen minutes with no answer the request
is denied, and the step says `Denied: no answer in 15 minutes`. Stopping
the run denies anything pending and stops it, as today. More than one
request can be pending at once, when an agent asks in parallel. Each
has its own buttons.

### Grants

A grant belongs to a repository, the `[[repo]]` identity, and is kept
in Perch's database with the words the card showed. A request is looked
up before it is shown. A grant that covers it answers it at once, with
the step `Allowed by a grant: …`. A grant covers a request when:

- `network:<host>`: the request names that host.
- `command:<text>`: the command is that text exactly. For Codex, that
  is the command inside `/bin/zsh -lc …`, the same text the card shows.
- `tool:<tool>:<what>`: the same tool on the same `<what>`.
- `commit`: a git command that writes the repository, as above. For
  Claude Code it also changes the sandbox of later runs (see Commits),
  so most commits never ask.

The item's summary on Conversation gains a row, **Work may**, on any
item that takes Work runs. It lists the repository's grants as chips
(`example.com`, `commit`, `cargo publish --dry-run`), each with a ×
that revokes it. The row is absent when there are none. Revoking takes
effect for the next request. A run already started keeps what its
sandbox was started with.

### Commits

The commit grant is the one grant that changes a sandbox rather than
answering a request. A Claude Code Work run in a repository with the
grant starts with the git directories of its worktree, the linked
worktree's own and the repository's, in `allowWrite`. `denyWrite`
keeps the repository's `config`, `config.worktree`, `hooks`, and the
worktree's `config.worktree` read-only, so one write cannot become a
program the reader runs later outside any sandbox. The framing tells
the agent it may commit and must not push.

Codex already commits (M17), so the grant changes nothing for Codex's
sandbox. Its framing says the same as Claude Code's.

### Trust

`[repo.work] trust = true` loads the repository's own agent
configuration for Work runs. Claude Code reads `--setting-sources
user,project,local`, and a `.mcp.json` in the worktree is passed with
`--mcp-config` instead of the empty one. Codex marks the worktree
`trusted` instead of `untrusted`, so its `.codex/` loads. Ask runs and
reviews ignore it and stay isolated. The README says what trusting a
repository hands to its files: settings, hooks and MCP servers there
run as the reader, outside any sandbox.

## Model

In `perch_agent`:

- `RunEvent::Approval { id, request: ApprovalRequest }` with
  `ApprovalRequest { kind: ApprovalKind, subject: String, detail:
  Option<String>, suggestion: Option<String> }`, where `ApprovalKind` is
  `Host`, `Command`, `Tool { name }` and `Edit`. `subject` is the host,
  the command, or the path. `suggestion` is the CLI's rule content.
- `Decision { Allow, Deny { message } }`, and `Run::answer(id,
  Decision)`, which writes the reply on the process's stdin.
- Two interactive transports beside the print one, picked by `Mode` and
  backend. **Claude host**: `-p --input-format stream-json
  --output-format stream-json --verbose --permission-prompt-tool stdio`.
  The prompt goes on stdin as a `user` message after an `initialize`
  control request, `can_use_tool` control requests become `Approval`
  events, and stdin is closed after `result`, which ends the process.
  **Codex app-server**: `codex app-server` with `--disable plugins
  --disable hooks --disable apps` and `-c mcp_servers.<name>.enabled=false`
  for each server in the user's Codex config. Then `initialize`,
  `initialized`, `thread/start` (or `thread/resume` with the session
  id) with `cwd`, `model`, `approvalPolicy: "on-request"`,
  `approvalsReviewer: "user"`, `developerInstructions`, and `config`
  holding the Work profile, trust and effort. Then `turn/start`.
  `item/*` notifications become the same steps `codex exec` gave, token
  usage becomes `Usage`, and `turn/completed` becomes `Finished` and
  closes stdin. The three approval requests become `Approval` events,
  answered `accept` or `decline`.
- Work drops bare `Bash` from Claude Code's allowed list. Sandboxed
  commands run through `autoAllowBashIfSandboxed`, and a command asking
  to leave the sandbox asks. With `Bash` allowed it would not have
  asked: checked against the CLI. The sandbox sets
  `allowUnsandboxedCommands: true` and no `strictAllowlist`, since both
  now mean "ask" rather than "refuse".
- `WorkGrant` gains `commit: bool` and `trust: bool`.

In `perch_github`:

- `WorkGrantRecord { id, repo, key, label, created_at }`, a table
  `work_grants` unique on `(repo, key)`, schema 27. `grants_for(repo)`,
  `insert_grant`, `delete_grant`.
- `RepoWork` gains `trust: bool`.

## Backend

- `execute_question` passes `Approval` events to `answer_or_ask`. It
  looks the request up in the repository's grants. On a match it
  answers `Allow`, records the step and emits nothing else. Otherwise
  it records the pending request, sets the activity, sends
  `Event::ApprovalRequested { run_id, approval_id, request }`, and
  arms a fifteen-minute timer that answers `Deny`.
- `Command::AnswerApproval { run_id, approval_id, answer: Once | Always
  | Deny }`. Always stores the grant first. The answer goes to the live
  `Run` through the handle the run manager keeps. The step is recorded,
  and `Event::ApprovalSettled` lets the card drop its buttons. An answer
  for a request already settled, by timeout or by a stop, is ignored.
- `Command::ListGrants { repo }`, `Command::RevokeGrant { id }`, and
  `Event::Grants { repo, grants }`.
- `work_setup` reads the commit grant and `trust` into `WorkGrant` and
  the framing.

## Harness

- Fake CLIs in shell scripts for both transports. The Claude one reads
  stdin, emits a `can_use_tool` control request, and prints what it was
  answered. The Codex one speaks enough JSON-RPC for initialize, a
  thread, a turn, one approval request, and a completed turn. Each
  records what Perch wrote to it.
- Pure tests: request parsing from recorded control requests and
  approval params, grant matching, the git-writes rule, the reply
  shapes, and the Claude Code and Codex command lines for Work with
  and without commit and trust.
- The per-worktree hold and the slot are held while waiting. The
  timeout denies. A stop settles pending requests.
- The migration from schema 26.
- By hand, against the real CLIs (an ignored test like M17's). Each
  agent is asked to `curl` a host and write outside the worktree. The
  host is allowed once and the write denied, and the disk and the
  output agree. The commit grant is given, and a Claude Code run then
  commits without asking and cannot write `.git/config`.

## Build order

1. `perch_agent`: `Approval`, `Decision`, `Run::answer`, stdin for the
   interactive transports. The Claude host transport with its fake and
   tests. Commit.
2. The Codex app-server transport with its fake and tests. Commit.
3. `perch_github`: grants table and migration. `perch_agent` Work
   settings for commit and trust. Commit.
4. Backend: approvals, grants, timeout, commands, events. Commit.
5. UI: the pending card, the settled line, the inspector activity, the
   **Work may** row with revoke. Commit.
6. The by-hand check against the real CLIs. README. Commit.

## Acceptance

In a worktree Perch made for this repository, with no `[repo.work]`:

- Ask a Claude Code Work run to fetch `https://example.com` with
  `curl`. The card asks `Connect to example.com`. **Allow once**: the
  command succeeds. Ask again in a new thread: it asks again.
  **Always**: it succeeds, the **Work may** row shows `example.com`,
  and a third thread's `curl` goes through without a card. Revoke it
  with ×: the fourth asks.
- Ask for `echo x > ~/perch-probe`. The sandbox refuses, the agent asks
  to run it outside, and the card shows the command. **Deny**: the file
  does not exist and the step says `Denied`.
- Ask it to commit. It asks to run `git commit` outside the sandbox,
  and the Always button reads **Always let Work commit here**. Take it.
  The next Work run commits without asking, and `git config` in that
  run fails.
- The same `curl` with Codex: an approval card, answered the same way,
  and the Codex run's activity shows its commands as before.
- Leave a request for fifteen minutes (a shorter constant in the test):
  it is denied and the run goes on.
- Stop a run that is waiting. It stops, and its request settles as
  denied.
- A question (Ask) on the same item never shows a card.
- With `trust = true` and a `.claude/settings.json` in the repository
  that allows `Bash(echo:*)`, a Claude Code Work run reads it: the init
  event lists the project source.

## Things the implementer should not do

- Do not write `.claude/settings.local.json`, `~/.codex/rules` or any
  other file of the CLIs' to remember a grant.
- Do not send `acceptWithExecpolicyAmendment` or `acceptForSession`.
- Do not let a Work run through with bare `Bash` allowed while
  unsandboxed commands are on.
- Do not answer a request the reader has not seen, except from a grant
  they gave.
- Do not use Codex's automatic reviewer, and do not start the user's
  MCP servers for a Work run.
- Do not change Ask runs or reviews.

## Afterwards: automatic reviewers by default

Changed after the milestone landed, at the reader's request. A Work run
now goes to each CLI's own automatic reviewer first, and to the reader
only for what that passes on: `[work] approvals = "auto"`, the
default, or `"ask"` for the behaviour above.

- Claude Code runs with `--permission-mode auto`. Its classifier
  decides sandbox escapes and host access itself: checked with Sonnet,
  an unsandboxed write to the home directory went through without a
  `can_use_tool` request. Haiku has no auto mode and falls back to
  asking. In auto mode `Bash` takes `allowed_domains`, which the
  classifier reviews with the command, and the step shows them.
- Codex runs with `approvalsReviewer: "auto_review"`. Its
  `item/autoApprovalReview/completed` notifications become steps:
  `Approved automatically: <command> (risk: low)` or `Refused
  automatically: … <rationale>`.
- Grants, the card and the **Work may** row are unchanged, and apply to
  whatever reaches Perch.
- This reverses the non-goal "Codex's automatic approval reviewer": the
  reviewer is the default now, and Perch records what it decided.
