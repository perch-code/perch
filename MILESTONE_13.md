# Perch: Milestone 13, Review a worktree

## Goal

Start Claude and/or Codex's native review workflow from Perch and read its
findings as inline AI discussions. Run another review after new commits,
a rebase, or a force push without losing the earlier review or filling the
diff with repeated comments about the same problem.

[Milestone 12](MILESTONE_12.md) supplies the run manager, Agents panel,
cancellation, and history infrastructure. This milestone adds review
admission, immutable source snapshots, native review adapters, persistent
findings, and reconciliation between reviews. Perch owns these records;
CLI conversation state is supplemental context.

## Scope

Review the full committed branch diff against an explicitly selected base.
Support Perch-managed worktrees and registered external worktrees, with or
without a GitHub pull request. Use the existing item ID as the history
owner. Acquiring a PR later does not create a new review history.

Uncommitted, staged-only, individual-commit, and incremental-only reviews
are later work. A dirty worktree is still reviewable: say `Committed
changes only; local edits are excluded`, and run against a captured
committed snapshot. Opening a different diff scope does not silently
change review scope. A worktree with no committed diff produces a clear
empty-scope message without launching an agent.

Results and follow-ups stay local to Perch. No posting, resolving, or
otherwise writing to GitHub; no automatic fixes, push-triggered reviews,
hosted review service, or new daemon. Kenn Forge remains a design reference.

## Behavior

### Review popover

Add Review to the worktree header. Clicking it opens a popover:

```text
Review branch
feature-branch against main
Committed changes only

Claude    [ High v ]
Codex     [ High v ]

                    [ Run review ]
```

Each backend gets its own dropdown with None plus supported review effort
levels for the installed CLI and selected model. None disables that
reviewer; it is not an effort passed to the CLI. Show the effective model
under its row, with a compact selector using Perch's configured model list.
An unavailable backend stays visible with a reason and None selected.

Remember model and effort choices per backend across launches, separately
from question-composer choices. On first use, select the available backend
of the existing default question model at a supported configured effort;
the other backend starts at None. If that backend is unavailable, use the
first available backend, Claude before Codex. Do not silently enable a
second reviewer. Preserve saved unavailable choices as preferences while
requiring a valid effective selection before launch.

The base starts from the item's configured base or PR base. For an external
branch with no known base, offer local refs and require a selection.
Show the base in the popover and allow changing it. Resolve refs to exact
commits on Run; do not silently fetch or choose a different base. If the
cached/local base is unavailable, explain what must be made available.

Run is disabled when both choices are None, no valid base is selected,
or a review group is already active for this item. Prevent duplicate
submission in the backend as well as in the UI. A newly created group
captures its choices; later preference changes affect only future groups.

### Progress and results

Run creates one review group and one reviewer run for each enabled backend.
Both use the same snapshot and execute concurrently when the shared
scheduler has room. The header shows `Reviewing · 1 of 2 complete` and
opens the group in Agents. Each child exposes queued/preparing/reviewing/
reconciling activity, elapsed time, tool steps, and Stop. Phases are status
labels, not estimated percentages. Stop all cancels every unfinished child.

Publish each reviewer's validated and reconciled findings when it finishes;
Claude does not wait for Codex. A second backend observing an existing
finding updates its attribution/history rather than creating another card.
Serialize reconciliation per item and commit it transactionally so two
concurrent completions cannot independently create the same finding.

A group summary appears in Conversation and review history:

```text
Review #3 · abc123 · against main at def456
Claude: completed · Codex: failed
2 new · 3 still present · 1 verified fixed · 1 not verified
```

Counts refer to unique findings and state their coverage. A failed backend
never counts as a clean review. Keep successful sibling results and offer
Retry for only the failed/stopped/interrupted child. Retry uses the original
snapshot and settings, creates a new attempt, and preserves the old one.
Review latest starts a new group against the current branch state. Disable
overlapping groups/retries for the same item while one is active; other
items and ordinary discussions may run concurrently.

Moving the branch during a run does not cancel it. Label its results with
the captured revision and `Newer changes exist`. Old completions cannot
replace a newer snapshot's projection or imply that the current branch is
fully reviewed.

### Inline discussions and history

A new finding opens a regular local AI thread at its validated location.
Use the existing stacked/split card renderer, gutter navigation, collapse
behavior, and follow-up composer. Show severity, reporting backend/model,
original review, and last verified revision. A reviewer can report an
unanchored concern; retain it in the summary rather than inventing a line.

Replies use the existing question flow and appear in Agents. Their context
includes the finding, its discussion, the original code, and any current
mapping. Sharing a thread does not require sharing a CLI session: start a
new session with explicit history when changing backends. Follow-ups do
not silently resolve a finding or alter review history.

By default, Files shows open findings confidently mapped to its current
snapshot. Fixed and dismissed findings remain accessible through filters
and review history, collapsed by default. Unmapped findings appear in the
file's outdated group or Conversation with View original code. Never attach
an outdated thread to an unrelated current line just because the numeric
line still exists. Use mappings appropriate to the displayed diff's two
sides; otherwise offer the original snapshot view.

History lists every group and reviewer attempt, including failures and
valid zero-finding reviews. Selecting a review opens its retained base/head
diff and observations as they were then. Opening a thread from history
shows the same ongoing discussion with original and subsequent entries
identified; later replies do not rewrite the historical observation.

## Snapshot and identity

At admission, capture the source item/repository identity, branch label,
requested base ref, resolved base tip, merge base, and head commit. Resolve
the diff once as merge-base-to-head. Branch names and PR numbers are useful
labels but cannot identify a snapshot. Record CLI versions, effective
settings, and review-adapter/output format versions with each attempt.

Materialize a private detached checkout in Perch-owned storage for that
snapshot. Both reviewers may read it concurrently; neither may edit it.
Capture from the source repository even for external worktrees. Keep the
review objects/source in Perch-owned storage so deleting the external
checkout or garbage-collecting its original clone cannot erase history.
Pin the required objects with private refs and retain a manifest plus the
exact diff. Store/pin each side of every historical anchor, including blobs
needed for renamed and deleted files. A SHA alone is insufficient retention.

An interrupted capture never starts a reviewer. If HEAD moves while
capturing, finish a consistent capture of the resolved commit or report
failure; do not combine versions. Give every native adapter immutable refs
and a clean checkout, so CLI defaults cannot add local edits or resolve a
moving branch. No reset, checkout, stash, or hook installation in the
reader's worktree. Honor the established read-only agent permissions in
the private checkout too.

Temporary checkouts can be removed after all associated processes finish.
Retained objects and manifests survive app restart and item archival.
Explicit item deletion removes its review history under the existing
delete policy; release shared objects only after their last reference is
gone. Cleanup must not remove snapshots belonging to another group/item.

## Native review adapters

Add a review operation to `perch-agent`, separate from an ordinary question.
Use the installed CLI's native review implementation. Do not silently
replace it with a generic review prompt if invocation or parsing fails.

The interfaces inspected on 2026-09-19 were:

- Codex 0.155.1 provides `codex exec review`, explicit base/commit scopes,
  JSONL events, and model/output options. Its dedicated review result is
  part of a session transcript.
- Claude Code 2.1.269 provides `/code-review` (`/review` is an alias) and
  supports noninteractive `-p` invocation. A normal local review waits and
  returns findings; JSON output alone does not promise a findings array.

Recheck installed help and official contracts during implementation. Build
capability detection and fixture coverage around supported versions. CLI
flags for base, custom instructions, and output schemas may not compose;
prove the chosen invocation honors the exact captured range before wiring
it into the UI. Unsupported versions/options leave the row unavailable
with an actionable reason, rather than silently changing the operation.

Define review-specific effort capabilities. Do not blindly reuse the
question flow's `Effort::Ultra`: Perch currently uses that for delegation,
while Claude's native `ultra` review can select a hosted service. Offer
supported local levels only, including None. Native local reviewer
subagents may need additional read/delegation tools; allow only what the
workflow requires while preserving write denial. Do not use bypass flags,
`--fix`, or `--comment`.

Normalize output into findings with a source output ID/index, title, body,
severity, file path, side, inclusive range when available, and supporting
evidence. Validate paths against snapshot membership and ranges against
the correct blob. Keep the original final response and diagnostic stream.
Treat code and review text as data, including any text resembling tool or
application instructions.

Prefer a verified structured native result when available. For text-only
results, use a versioned extraction adapter. If semantic extraction needs
an additional model call, track it as work within the reviewer attempt,
account for its usage, preserve its input/output, and require evidence
spans tying each finding to the native response. Extraction may structure
findings, not invent new ones. Unparseable or partly invalid output is
visible as an incomplete result with diagnostics; it must never become
`No findings`. Only an explicit, successfully parsed clean result earns
that label.

## Persistent findings and repeated reviews

Separate the review that ran from the problem it found:

| Record | Responsibility |
| --- | --- |
| Review group | Item, immutable snapshot, selected reviewers, lifecycle, summary |
| Reviewer attempt | Milestone 12 run, backend/settings, raw result, retry lineage, ingestion state |
| Finding | Stable item-local identity, existing discussion thread, user disposition |
| Observation | One attempt's report or verification of that finding at a snapshot, with original anchor and evidence |
| Anchor mapping | Finding location projected to another snapshot, mapping method and confidence |

Original results and observations are append-only. User actions such as
dismissal, reopening, and correcting a duplicate association retain an
audit entry. Never overwrite an old comment to make it describe new code.

Every repeat review first discovers findings independently, then verifies
prior relevant findings and reconciles the results. Do not put a blanket
`Do not repeat these findings` instruction in the discovery pass: that
could hide an unfixed bug. Verification is separately identified work
under the attempt, with progress, cancellation, and usage visible.

Build candidate matches using renamed/moved code, mapped ranges, affected
symbols, evidence, and the failure being described. Line numbers, commit
SHAs, and hashes of comment wording are not finding identities. Fingerprints
narrow the candidate set; they cannot establish semantic equality alone.
The same location can contain two unrelated problems.

Use deterministic matches when evidence is sufficient and a bounded
semantic comparison for plausible ambiguous candidates when needed. Record
the association and its evidence. Commit only validated associations;
serialize matching against the latest persisted findings, including those
just created by the other backend. A repeated observation adds a compact
`Seen in reviews #1, #3`/backend attribution entry, not another copy of the
same discussion message. New evidence or a materially different explanation
can add a message to the existing thread.

When identity remains uncertain, retain a separate finding marked Possible
duplicate with a link to the candidate. Offer Link to existing finding and
undo that association without losing either original report or replies.
Conservative ambiguity is preferable to merging unrelated bugs. Do not
promise perfect semantic deduplication.

Make ingestion idempotent with an attempt/output identity and transactional
observation/thread creation. Replayed completion events and app recovery
must not duplicate cards or summary counts. A retry is a new attempt but
reconciles against the same stable findings. After a crash between saving
valid output and ingestion, finish local ingestion from the saved result;
do not silently launch another model call.

### Location, verification, and disposition

Track location independently from whether a finding needs attention.
Location is mapped or outdated/unmapped. Findings are open, verified fixed,
or dismissed, with last verification revision and a separate not-verified
indicator for runs that could not check them.

- A fresh observation of the same problem keeps its thread open.
- A changed/deleted line or absence from the new result does not prove a
  fix. Mark verified fixed only after an explicit check with recorded
  evidence against the new snapshot. Scope changes or missing context can
  leave it not verified.
- User dismissal keeps a reason and wins over automatic reopening. Later
  observations stay attached and visible in history without bringing the
  card back into the default open list. Reopening is an explicit action.
- A previously verified-fixed problem that returns can reopen the same
  finding, with a recurrence entry and its new observation.
- If reviewers disagree, retain both observations, keep the finding open,
  and show the disagreement. One backend's failed or missing verification
  cannot resolve another backend's finding.

Base changes and force pushes require fresh mapping/verification, without
assuming the previous head is an ancestor. Stable item/finding IDs survive
both. Verification remains qualified by snapshot; a result from an older
snapshot cannot resolve an issue on the current branch automatically.

## Storage and implementation

Add review records and indexed lookups in `perch-github`, where Perch's
current local data lives. Use the preserving migrations introduced in
Milestone 12. Keep one discussion model for ordinary questions and review
findings. Extend anchors with explicit snapshot/side identity and separate
projections instead of repeatedly mutating `Anchor.commit_sha` and lines.
The current file-wide outdated check is insufficient for review findings;
it may trigger mapping work but must not declare a fix.

Put capture, object retention, detached checkout preparation, and line
mapping in `perch-git`. Keep snapshot-to-snapshot mapping testable without
a UI. Put invocation/parsing in `perch-agent`; put review orchestration
and reconciliation in focused app backend modules using Milestone 12's
run manager. Keep admission and result ingestion independent of whichever
item is currently open.

Add the popover to the worktree header, review-group entries to Conversation
and Agents, and history/original-snapshot navigation to the review view.
Opening a historical snapshot is read-only and does not change the source
checkout or the item's current-head metadata.

Archive review summaries, findings, and their discussions through
`perch-knowledge` with stable IDs, backend attribution, and snapshot links.
Repeated export must not duplicate an unchanged message. Keep original
observations distinguishable from later disposition and follow-up entries.

## Build order

1. Define review records, preserving migrations, ownership, and group/attempt
   state transitions on top of Milestone 12.
2. Implement immutable capture, retained objects, private checkouts, and
   historical diff loading for managed and external worktrees.
3. Implement capability-aware native review adapters and validated output
   ingestion. Capture real versioned fixtures without making live calls a
   requirement of the automated suite.
4. Add Review/None/effort/model controls, admission, concurrent reviewer
   progress, stop/retry, and raw-result inspection in Agents.
5. Create inline finding threads, review summaries, history, and original
   code navigation. Wire both stacked and split rendering.
6. Add mapping, prior-finding verification, semantic reconciliation,
   idempotent ingestion, dispositions, and duplicate-association correction.
7. Add knowledge export and README documentation of scope, history,
   unsupported CLI behavior, and retry versus Review latest.
8. Complete the acceptance checks, then run `cargo test` and
   `cargo clippy --all-targets` after the implementation and docs are complete.

The milestone is complete only when repeat reviews work. A first-review
demo that produces inline cards is an intermediate step.

## Acceptance

- Select Claude alone, Codex alone, both, and both None in the Perch UI.
  Verify effort/model choices, disabled states, remembered preferences,
  missing binaries, unsupported versions, and duplicate-click admission.
- Review a managed branch with no PR and an external registered worktree.
  Dirty files stay untouched and excluded. Both backends see exactly the
  captured committed range even if the source branch changes during the run.
- Finish one reviewer while the other runs or fails. Findings appear once,
  progress remains accurate, and the summary states partial coverage. Stop
  one or all reviewers, including during extraction and verification.
- Repeat an unchanged review with reworded findings and findings shared by
  both backends. Keep one thread per confidently matched problem, original
  replies, both attributions, and every review/attempt in history.
- Insert lines, rename a file, rebase, force-push unrelated history, delete
  an affected file, and change the base. Map only with sufficient evidence;
  unmapped findings still open their original code. Two bugs on the same
  line must not merge. An ambiguous duplicate can be linked and unlinked.
- Fix one problem and leave another. Explicit verification records the fix;
  missing output alone does not. Test dismissal persistence, recurrence,
  disagreement, and older-snapshot results arriving after branch movement.
- Replay a completion event and interrupt ingestion at transaction
  boundaries. Restart without duplicate observations/cards or unrequested
  model calls. Failed parsing never reports a clean review.
- Retry one failed backend against the original snapshot, then run Review
  latest. Both attempts and both snapshots remain distinct and readable.
- Remove the source checkout and prune unreachable objects in its original
  repository. Historical diffs and anchors remain readable from retained
  storage. Item archival retains them; explicit deletion releases only its
  unshared data after associated processes end.
- Follow up on a finding with either backend. The existing thread and
  Agents panel agree about state and attribution. New replies do not alter
  what an earlier review reported.
- Exercise header/popover keyboard focus, long findings, stacked/split
  anchors including deleted/base-side code, outdated groups, and history
  navigation in the Floem harness and native window. Automated tests use
  fixture reviewers and real temporary Git histories; live CLI smoke checks
  verify the supported native commands separately.

## References

- [Codex native review](https://learn.chatgpt.com/docs/code-review?surface=cli)
  describes review scope and transcript behavior.
- [Claude local review](https://code.claude.com/docs/en/code-review#review-a-diff-locally)
  describes local versus hosted modes and noninteractive output. Reverify
  these moving interfaces at implementation time.
- [Kenn Forge inline review comments](https://github.com/kenn-io/forge/blob/main/context/inline-review-comments.md)
  binds drafts to a diff head and avoids attaching stale comments by numeric
  coincidence. Use those principles without adopting its PR publication
  model or disabling Perch's local worktree discussions.
- [Kenn Forge review integration](https://github.com/kenn-io/forge/blob/main/docs/integrations.md)
  keeps job ownership explicit. Perch's run manager and review records own
  the jobs and history in this design; no Roborev dependency is introduced.
