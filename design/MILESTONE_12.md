# Perch: Milestone 12, Agents across the app

## Goal

See and control the agents Perch is running, whichever item is open.
An answer to an inline question, a follow-up, and the worktree reviews in
[Milestone 13](MILESTONE_13.md) belong in one Agents panel. Each run shows
its worktree, purpose, backend, model, effort, elapsed time, and current
activity. The reader can open its discussion or result and stop it.

Perch keeps its Rust backend and native Floem UI. Kenn Forge is a reference
for activity and lifecycle behavior, not a dependency or a replacement
backend. This milestone ships with the existing question and follow-up
flows; it does not wait for automated reviews.

## Scope

Include all top-level agent runs that Perch launches. Agents started from
other applications are outside this list. Backend subagents appear under
their parent when the CLI identifies them; they do not inflate the count
of top-level runs.

The panel is for activity, results, and control. New conversations still
start from the existing composers. This milestone does not add a general
coding terminal, arbitrary command launcher, background daemon, remote
execution, or an approval console. Agents retain their current read-only
permissions. A permission denial remains visible in the activity log.

## Behavior

### Entry point and panel

Put an Agents control in persistent app chrome, available with no item
open and while moving between items. Show `Agents · 2 running` when two
runs are executing, with a separate queued count when needed. A request
to stop continues to count as active until the process has actually ended.
With no active work, the control reads `Agents`; history remains available.

Clicking it opens a resizable app-level panel. It is independent of the
open item's thread column. Opening it preserves the file, selection,
scroll position, and composer draft. Closing it stops nothing. Remember
the panel width and open/closed preference across launches.

The panel has Active and History views. Active groups runs by worktree,
with the current item's group first and the remaining groups in stable
start order. Within a group, show active work before queued work. History
lists terminal runs newest first and loads older rows on demand. Offer
filters for current worktree/all worktrees and Claude/Codex/all backends;
History also filters by outcome. The global running count is never filtered.

A row has:

- Backend, model, and effort, using the values actually used by the run.
- Purpose: an abbreviated question, Follow-up, or Worktree review.
- Repository and branch/item label, with a link to the source.
- State, elapsed time, and the latest meaningful tool activity.
- View and Stop while active; View and Retry for failed, stopped, or
  interrupted work. A queued row offers Cancel.

Selecting a row opens its detail within the panel: the request, ordered
tool steps, streamed response, final result or error, and reported usage.
Reuse the current run/markdown rendering where possible. Auto-follow new
output only while the reader is at the bottom. Missing cost or token data
is unavailable, not zero. Do not invent a completion percentage from time
or tool-call counts. Distinguish time spent queued from execution time.

Review groups added in Milestone 13 can expand into their individual
reviewer runs. Group headers are not additional agents.

### View, completion, and retry

View opens the owning item and focuses its exact discussion or review.
Queue that focus until the item's asynchronous load has completed. A late
event from a different run must not steal navigation or focus. Archived
items can be viewed without silently unarchiving them. If a checkout has
gone away, retained requests and results remain readable in the panel.

Completion updates the count and shows a small in-app notice with a View
action. It never opens another item automatically. A paired review emits
one group completion notice, with each backend's outcome available inside.
No operating-system notifications or sounds are required.

Retry creates a new run ID linked to the old run. Keep the failed attempt,
its output, and its timestamps. Retrying a question reuses its original
request and thread without adding another copy of the user's message;
show each attempt under that request. Permit only one active answer in a
thread. Validate the original backend/model and report unavailable choices
instead of silently substituting another model. Reviews define their
snapshot retry rules in Milestone 13.

### Lifecycle

Use explicit states: Queued, Starting, Running, Stopping, Completed,
Failed, Stopped, and Interrupted. Queued cancellation goes directly to
Stopped without spawning. Stopping an executing run cancels its process
tree, drains available output for a bounded interval, and reaps the child
before reaching Stopped. Do not leave grandchildren running after the
panel says they stopped.

Terminal transitions are idempotent. Cancellation racing with a completed
process preserves a completion already committed; late output cannot
resurrect a run. A process exit without a valid completion event is a
failure, not success. Distinguish a spawn failure, a permission denial,
and a user stop in the UI without deriving state from an error string.

Use a shared scheduler with a configurable limit, initially four executing
top-level runs. Queue additional requests in FIFO order. Native subagents
are accounted for under their parent and remain controlled by that CLI.
Persist a queued request before acknowledging it and persist Starting
before spawning, so a crash cannot leave an invisible request.

Navigating, changing scope, and closing the Agents panel do not cancel
work. Archiving an item retains its runs. Explicit thread/item deletion
must cancel and await affected work before deleting its records or
checkout; late events must not recreate deleted discussions. Keep existing
deletion confirmation behavior rather than adding another approval step.

Perch owns the child processes for this milestone. Normal app shutdown
cancels and reaps them with a bounded grace period. On restart, reconcile
persisted nonterminal runs to Interrupted and retain their partial output;
do not automatically replay requests or launch paid work. Design process
ownership so a parent crash cannot leave unmanaged children consuming
resources. A saved CLI session ID is context for a future invocation, not
proof of a running process. Reattachment across app restarts is out of scope.

## Model and ownership

The current `RunRecord` in `perch-github::comments` requires a `thread_id`,
and `backend::Ctx.runs` maps a process handle to a thread. Generalize these
into app-wide runs with an explicit owner. Keep the data model free of
Floem and the execution layer free of database dependencies.

Each run records:

- Stable run ID and owner: item ID plus thread/request ID, or a review
  reviewer-run ID when Milestone 13 lands. A branch label is display data,
  not identity.
- Purpose, backend, executable/version, requested and effective model and
  effort, CLI session ID when available, and original request.
- Working directory and available revision context; questions using a
  live checkout must not claim to have reviewed an immutable snapshot.
- State, queue/start/end timestamps, terminal reason, reported usage,
  retry-of ID, ordered events, partial output, and final result.

Store owner labels with the run so history can render without opening the
item. Validate ownership when acting on it. Keep one authoritative run
record; discussion messages reference it rather than maintaining separate
copies of its state. A CLI session can be resumed by multiple successive
runs and must never replace the Perch run ID.

The backend run manager owns admission, scheduling, process handles,
cancellation, persistence, and completion dispatch. It captures request
context when admitted instead of depending on `Ctx.slot` still naming the
same item when a queued request starts. It must work while another item is
open or no item is open.

`AppState` holds summaries and panel selection. `ReviewState` continues to
hold the open item's display. Apply every run event to the app-wide state,
then update the open discussion only when its owner matches. Opening an
item loads the latest persisted run state, including activity that arrived
while it was elsewhere.

Give events a per-run sequence and make replay/application idempotent.
Persist accepted events before publishing them to the UI; bound individual
payloads and paginate old events rather than growing an unbounded signal.
Startup loads active summaries and a page of history, not every transcript.

## Storage

Add indexed queries for active runs, paginated history, one run's events,
and runs belonging to an item or discussion. Preserve the existing run IDs,
message associations, session IDs, and tool history when upgrading.

Schema 13 currently recreates the database on a version mismatch. Replace
that path with transactional, versioned migration for the current schema
and these milestones. On migration failure, retain the original database
and report the error. A newer or unsupported schema must not be deleted.
Earlier milestones' instructions to bump and recreate do not apply here:
the database contains local conversations and will contain review history.

Existing runs become discussion-owned records. Derive item ownership from
their threads; preserve an orphaned legacy record as unavailable history
rather than inventing an owner. Old completed runs need not acquire data
the old model did not store, such as the CLI version or token usage.

## Implementation boundaries

- `perch-agent`: normalized process events and typed completion/cancel
  outcomes; process-tree lifetime; retain Claude and Codex adapters.
- `perch-github`: durable run types, migrations, indexed history, and
  discussion references, following the current storage location.
- `perch-app::backend`: extract run coordination from the large backend
  module into a focused manager. Keep question prompt construction in
  `ask`; route the existing Ask and CancelRun commands through the manager.
- `perch-app::state` and a new Agents view module: app-wide summaries,
  filters, panel details, navigation, and completion notices.
- Existing inline cards: read the shared run state and preserve their
  current stop, step-log, and follow-up behavior.

Do not add an HTTP server or dependency on Forge, Roborev, tmux, or hooks
installed into the user's CLI configuration. Perch already receives events
from the processes it launches.

## Build order

1. Introduce the owner-aware run model, event storage, and preserving
   migration. Adapt existing discussion reads without changing the UI.
2. Extract the run manager and route questions/follow-ups through it.
   Capture request context, add admission limits, and implement typed
   cancellation, shutdown, and restart recovery.
3. Add app-wide event application and run summaries. Make existing inline
   cards use the same state; navigation must not affect execution.
4. Add the persistent Agents control, panel, filters, detail, navigation,
   notices, and retry. Wire deletion to cancellation completion.
5. Update README with the UI and process-lifetime behavior. Add a read-only
   `perch --agents` dump for the saved summaries and ownership, using the
   same queries as the panel; it does not launch agents.
6. Complete the acceptance checks below, then run `cargo test` and
   `cargo clippy --all-targets` after implementation and documentation edits.

## Acceptance

- Start Claude on one item and Codex on another. Both appear in Agents,
  continue while browsing a third item, and update only their own threads.
  View focuses the right thread after its item finishes loading.
- Exceed the admission limit. Queued runs show their order; canceling one
  never spawns it. Double submission cannot create two active attempts for
  the same discussion request.
- Stop a running process with a child process holding an output pipe.
  Both terminate, the panel leaves Stopping, and available output survives.
  Exercise stop/completion races and duplicate terminal events.
- A missing executable, malformed stream, permission denial, and unexpected
  exit each leave an intelligible result and no permanently running row.
- Retry a failed answer. The thread retains one user question and both
  attempts, and only the successful attempt supplies the completed answer.
- Quit with work queued and running, then relaunch. No process remains
  unmanaged, history survives, and nothing starts automatically. Exercise
  abnormal termination as well as normal shutdown with fixture processes.
- Upgrade a schema-13 database containing discussions, messages, and runs.
  Verify IDs and content survive; a failed migration does not erase data.
- Archive, delete a thread, and delete an item during activity. Verify each
  stated lifecycle behavior and reject late writes after deletion.
- In the Floem harness, open/close the panel during streaming, filter it,
  navigate to a result, and use its controls by keyboard. Focus returns to
  the opener, text selection works, and file scroll/drafts survive.
- Inspect the native window at narrow and wide sizes. Long branch names,
  unavailable usage, empty history, and long logs remain readable. Use fake
  backends for automated checks; live model calls are an explicit smoke
  check, not a requirement for the test suite.

## Design reference

[Kenn Forge's runtime lifecycle](https://github.com/kenn-io/forge/blob/main/context/workspace-runtime-lifecycle.md)
separates persisted workspace identity from live process/session state.
[Its workspace contracts](https://github.com/kenn-io/forge/blob/main/context/workspace-apis.md)
also distinguish agent activity from provider-item activity. Keep those
distinctions in Perch. The terminal transport and workspace ownership
policies are Forge's and are not part of this milestone.
