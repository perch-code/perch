# Perch: Milestone 6, Threads

## Goal

Let the reader ask a coding agent about the code on screen, and keep the
answer where the question was asked.

Today Perch shows a pull request's files and diff, and rust-analyzer says
what a symbol is. Anything more than that means switching to a terminal,
starting Claude Code in the worktree, and typing the file and line by hand.
The answer lands in a scrollback nobody will find again.

After this milestone the reader selects some lines, presses a key, types a
question, and the answer appears beside the code as a thread anchored to
those lines, the way a review comment is anchored on github.com. A thread
takes follow-ups. It is stored in `perch.db`, so reopening the worktree
shows it again with a marker in the gutter. Each question chooses its model
and effort.

Claude Code is the only agent this milestone talks to. The run is a child
process, `claude -p` in the worktree with the question on the command line
and JSON events on stdout. Perch shows what the agent did, which files it
read and what it ran, and what it said.

The tables are shaped so that review threads fetched from GitHub, and
replies to them, can sit in the same list later. Nothing in this milestone
fetches or posts one.

## Non-goals

- Codex. One agent, one event format, until the shape of a run is settled.
  The backend enum has one variant and is expected to grow.
- `/review` or any run without a question. A review is a run with a preset
  prompt and no anchor, and it comes after the run machinery exists.
- The agent changing the worktree. The run is read-only by construction:
  the tools it may use are listed, and edits are not on the list.
- GitHub review threads. Not fetched, not shown, not replied to. The
  `origin` column is there for them; no row with that origin is written.
- Re-anchoring. A thread is marked outdated when its lines may have moved.
  Following them to their new position is later.
- Language server context in the prompt. The agent reads the files itself.
  Hover and definition data would sharpen the answer, and there is a slot
  for it, but the first version measures how far the agent gets alone.
- Perch as a tool server. The agent cannot drive Perch. That is its own
  milestone, and it wants a few hundred real threads first to know which
  actions matter.
- Token-by-token streaming. Events arrive per message and per tool call.
  That is enough to see the agent working. Partial text can be turned on
  later with one flag.
- A chat window. A thread is a comment thread: short questions, short
  answers, tied to lines. A long open-ended session belongs in the terminal
  and the header's Open in Terminal is still the way to get one.
- Editing in Perch. Selecting text is new; changing it is not.

## Behaviour

In a review, the reader selects lines in the editor by dragging, or
shift-clicking a second line, or clicks the `+` that appears on hover in
the gutter. Pressing `a`, or clicking the `+`, opens the thread column on
the right if it is closed and puts a composer in it with the anchor shown
above the text box: the file name and the line range. The composer has a
model and an effort picker, defaulting to what `config.toml` says, and a
Send button. `Cmd-Enter` sends.

Sending starts a run. The composer becomes the first message of a new
thread, and under it a running block shows what the agent is doing, one
line per tool call: "Read src/manifest.rs", "Ran git log -5 -- src/". When
the agent answers, its text renders under the tool lines, which collapse
to a single "Read 3 files, ran 1 command" line that expands on click. The
thread's footer says the model, the effort, how long it took, and what it
cost.

A thread takes a follow-up from a composer at its bottom. A follow-up
resumes the agent's session, so the agent remembers the earlier exchange
without Perch resending it. The model and effort pickers stay per message;
changing them for a follow-up is allowed and the footer of each answer says
what it used.

A run can be cancelled. The block says "Stopped" and the thread stays,
with whatever tool lines arrived before the stop.

A run can fail: the binary is missing, the agent is not logged in, the
process dies. The block says why in one line, and the question stays so it
can be sent again.

A thread with no anchor is allowed. Pressing `a` with nothing selected, or
the `+` at the top of the thread column, opens a composer whose anchor line
says the worktree name. This is how a question about the pull request as a
whole is asked. It costs nothing extra to support and is the first thing
someone will try.

Anchored threads show a marker in the gutter on their first line, beside
the change bar. Clicking the marker scrolls the thread column to that
thread. The thread column lists the open file's threads first, then the
worktree's other threads under a divider, newest first within each. A
thread in the second group names its file and clicking it opens the file.

When the worktree's head has moved since a thread was made, and the
thread's file differs between the two commits, the thread is outdated. It
says so in a small line under its anchor and its gutter marker moves to
the muted colour. It still takes follow-ups; the agent is told the lines
may have moved.

The column is shown or hidden with `Cmd-Shift-\`, matching `Cmd-\` for the
drawer on the left. It opens itself when a thread is started and when a
file with threads is opened, and stays where it was put otherwise.

Nothing here touches GitHub. The pull request title and body that go into
the prompt are already in hand from opening the review.

## The run

A run is one invocation of the agent in the worktree:

```
claude -p <question>
  --output-format stream-json --verbose
  --model <model> --effort <effort>
  --append-system-prompt <framing>
  --allowedTools Read Glob Grep LS "Bash(git diff:*)" "Bash(git log:*)" "Bash(git show:*)" "Bash(git blame:*)" "Bash(rg:*)"
  --permission-mode default
  [--resume <session_id>]
```

with the worktree path as the working directory. The tool list is the whole
permission story: in print mode a tool not on the list is denied without
asking, and the denial arrives as an event, so the thread can say "asked to
edit src/x.rs; not allowed" rather than hang.

The first question of a thread sends the framing as an appended system
prompt and the anchor as the head of the user message:

```
You are answering a question inside a code review tool. The reader is
looking at a pull request checked out in the current directory. Answer
the question about the lines given; read whatever else you need. Be
brief. Do not change any file.
```

```
Pull request: <owner/repo>#<n>: <title>
<body, if any, trimmed to 40 lines>

File: src/manifest.rs, lines 41–58 (changed in this pull request)
```
```rust
<the selected lines>
```
```
Question: <what the reader typed>
```

"changed in this pull request" appears when the selection overlaps a hunk,
and the hunk's removed lines follow the selection as a second block headed
"Before this pull request". For a thread with no anchor the file block is
omitted. For an outdated thread a line says the file has changed since the
thread began and the numbers may be off.

A follow-up passes `--resume` with the session id the first run reported
and sends only the new question. The agent holds the conversation.

Events, as the process writes them, one JSON object per line:

- `system` with subtype `init`: carries the session id and the model. This
  is when the run has started.
- `assistant`: a message with content blocks. A `text` block is something
  the agent said. A `tool_use` block is a tool call, with its name and
  input.
- `user`: a `tool_result` block, the outcome of a tool call. A denied tool
  arrives here, as an error result.
- `result`: the end. Subtype `success` or an error subtype, with the
  session id, cost in dollars, duration, and the final text.

Anything else is ignored. The first build step confirms these names
against the installed binary before anything depends on them.

## Model

A new crate, `perch-agent`, that knows how to run an agent process and
read its events. No dependency on `perch-github` or `perch-app`.

```rust
pub enum Backend { ClaudeCode }

pub enum Effort { Low, Medium, High, XHigh, Max }

pub struct RunSpec {
    pub backend: Backend,
    pub model: String,              // "sonnet", "opus", or a full id
    pub effort: Effort,
    pub cwd: PathBuf,
    pub prompt: String,
    pub system: Option<String>,     // appended, first run only
    pub resume: Option<String>,     // session id from an earlier run
    pub allowed_tools: Vec<String>,
}

pub enum RunEvent {
    Started { session_id: String, model: String },
    Text(String),
    ToolCall { id: String, name: String, summary: String },
    ToolResult { id: String, ok: bool, summary: String },
    Finished { session_id: String, cost_usd: Option<f64>, duration_ms: u64 },
    Failed { message: String },
}

pub struct Run { /* child handle */ }

impl Run {
    pub fn spawn(spec: RunSpec, events: mpsc::Sender<RunEvent>) -> Result<Run, Error>;
    pub fn cancel(&self);   // kill the child; a Failed { "Stopped" } follows
}

pub fn summarize_tool_call(name: &str, input: &serde_json::Value) -> String;
```

`summarize_tool_call` turns `Read { file_path }` into "Read src/x.rs" and
`Bash { command }` into "Ran git log -5", trimmed to one line. It is where
the tool lines in the UI come from, and it is a pure function with tests.

The parser is a function from a line of text to `Option<RunEvent>`, also
pure, also tested, against fixture lines captured from the real binary.

The thread model lives in `perch-github` beside `Worktree` and `Mark`,
because the cache does and the types are what the cache reads and writes:

```rust
pub enum ThreadOrigin { Local, GitHub }
pub enum Side { Left, Right }         // GitHub's names: old file, new file

pub struct Anchor {
    pub path: String,
    pub start_line: u32,              // inclusive, in the file on `side`
    pub line: u32,                    // inclusive; equal to start_line for one line
    pub side: Side,
    pub commit_sha: String,           // the head the lines were read at
}

pub struct CommentThread {
    pub id: String,                   // ULID for local, node id for GitHub
    pub worktree_id: String,
    pub origin: ThreadOrigin,
    pub anchor: Option<Anchor>,
    pub outdated: bool,
    pub resolved: bool,
    pub created_at: Timestamp,
    pub messages: Vec<CommentMessage>,
}

pub enum Author { User, Agent { backend: String, model: String }, GitHubUser(String) }

pub struct CommentMessage {
    pub id: String,
    pub thread_id: String,
    pub origin: ThreadOrigin,
    pub author: Author,
    pub body: String,
    pub created_at: Timestamp,
    pub run: Option<RunRecord>,       // an agent's answer carries its run
}

pub struct RunRecord {
    pub id: String,
    pub backend: String,
    pub model: String,
    pub effort: String,
    pub session_id: Option<String>,
    pub status: RunStatus,            // Running, Finished, Failed, Stopped
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub steps: Vec<RunStep>,          // tool lines, in order
    pub error: Option<String>,
}

pub struct RunStep { pub summary: String, pub ok: bool }
```

`CommentThread` rather than `Thread`, because `Thread` is already the
notification. `side` and `start_line` are GitHub's shape on purpose. A
local thread is always `Right`, and a review comment on a removed line will
be `Left` when those arrive, and the one anchor type serves both.

## Storage

Three tables beside `worktrees` in `perch.db`:

```sql
CREATE TABLE IF NOT EXISTS comment_threads (
    id             TEXT PRIMARY KEY,
    worktree_id    TEXT NOT NULL,
    origin         TEXT NOT NULL,          -- 'local' | 'github'
    github_id      TEXT,                   -- review thread node id; NULL for local
    path           TEXT,                   -- NULL for a thread with no anchor
    start_line     INTEGER,
    line           INTEGER,
    side           TEXT,                   -- 'left' | 'right'
    commit_sha     TEXT,                   -- head the anchor was made at
    outdated       INTEGER NOT NULL DEFAULT 0,
    resolved       INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS comment_threads_worktree ON comment_threads(worktree_id);

CREATE TABLE IF NOT EXISTS comment_messages (
    id             TEXT PRIMARY KEY,
    thread_id      TEXT NOT NULL REFERENCES comment_threads(id) ON DELETE CASCADE,
    origin         TEXT NOT NULL,
    github_id      TEXT,                   -- comment id once posted; NULL for local
    author_kind    TEXT NOT NULL,          -- 'user' | 'agent' | 'github_user'
    author         TEXT,                   -- login, or the agent backend
    body           TEXT NOT NULL,
    run_id         TEXT REFERENCES runs(id),
    created_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS comment_messages_thread ON comment_messages(thread_id);

CREATE TABLE IF NOT EXISTS runs (
    id             TEXT PRIMARY KEY,
    thread_id      TEXT NOT NULL,
    backend        TEXT NOT NULL,          -- 'claude-code'
    model          TEXT NOT NULL,
    effort         TEXT NOT NULL,
    session_id     TEXT,
    status         TEXT NOT NULL,          -- 'running' | 'finished' | 'failed' | 'stopped'
    cost_usd       REAL,
    duration_ms    INTEGER,
    steps_json     TEXT NOT NULL DEFAULT '[]',
    error          TEXT,
    started_at     TEXT NOT NULL,
    ended_at       TEXT
);
```

The rule the columns encode: rows with origin `github` are a cache that a
future fetch may replace wholesale by `github_id`; rows with origin `local`
are the truth and nothing replaces them. A reply to a GitHub thread, when
that comes, is a `local` message inside a `github` thread that gains a
`github_id` when posted. That is why origin is on the message and not only
on the thread.

Runs are their own table so a message stays a message. Steps are a JSON
array in one column: they are only ever read with their run, and a run has
a dozen of them.

A `running` row at startup is a run Perch died in the middle of. It is
marked `failed` with "Perch quit" on open. Nothing tries to reattach.

Cache methods:

- `load_comment_threads(worktree_id) -> Vec<CommentThread>`, with messages
  and runs joined in.
- `insert_comment_thread(&CommentThread)`
- `insert_comment_message(&CommentMessage)`
- `insert_run(&RunRecord)`, `update_run(&RunRecord)`
- `set_thread_outdated(id, bool)`
- `delete_comment_thread(id)`
- `fail_running_runs(reason)`, for startup.

`Worktree::forget` cascades: forgetting a worktree deletes its threads.
Archiving does not; the threads are the point of keeping the record.

## Backend

Commands:

```rust
Ask {
    worktree_id: String,
    thread_id: Option<String>,      // None starts a thread
    anchor: Option<Anchor>,         // ignored on a follow-up
    question: String,
    model: String,
    effort: Effort,
},
CancelRun { run_id: String },
DeleteCommentThread { id: String },
```

Events:

```rust
CommentThreads { worktree_id: String, threads: Vec<CommentThread> },
RunEvent { run_id: String, thread_id: String, event: perch_agent::RunEvent },
```

Sequence:

- `ReviewReady` is followed by `CommentThreads` from the cache, after the
  outdated check below. Threads paint after the code, not before.
- `Ask` with no thread: insert the thread and the user's message, insert a
  `running` run, emit `CommentThreads`. Build the prompt from the anchor,
  the file, and the pull request. Spawn the run on a task with the worktree
  path as cwd. Forward each `RunEvent` as it arrives, appending steps to the
  run row as they come so a crash mid-run loses nothing shown. On
  `Finished`, insert the agent's message with the collected text, update
  the run, emit `CommentThreads`. On `Failed`, update the run with the
  error and emit `CommentThreads`.
- `Ask` with a thread: the same, with `resume` set to the session id of the
  thread's last finished run and no system prompt. If no run in the thread
  finished, start fresh with the full framing and the thread's earlier
  messages folded into the prompt, so a thread whose first run failed is
  not stuck.
- `CancelRun`: kill the child. The `Failed { "Stopped" }` that follows sets
  the row to `stopped`.
- `DeleteCommentThread`: cancel any running run in it, delete, emit.
- Outdated check, on open: for each anchored thread whose `commit_sha`
  differs from the worktree's head, run `git diff --quiet <sha> <head> --
  <path>` in the worktree. Non-zero means the file changed; set outdated.
  One process per thread per open is fine at the counts involved, and the
  check is skipped when the shas match, which is the common case.

The model and effort pickers read from config and the last used values.
The agent binary is found on `PATH`, overridable in config. A missing
binary is a `Failed` event with a message that names the config key.

Runs are not serialised. Two threads can run at once; each is a process.

## Config

```toml
[agent]
binary = "claude"              # optional; found on PATH by default
model = "sonnet"               # optional; default for the picker
effort = "medium"              # optional; low | medium | high
models = ["sonnet", "opus"]    # optional; what the picker offers
```

`Config` grows `agent: AgentConfig` with serde defaults, as `knowledge`
did. An existing `config.toml` keeps working.

## State

`ReviewState` gains:

```rust
pub comment_threads: RwSignal<Arc<Vec<CommentThread>>>,
pub threads_open: RwSignal<bool>,
pub composer: RwSignal<Option<Composer>>,   // an open composer and its anchor
pub selection: RwSignal<Option<(usize, usize)>>, // display lines, from the editor
pub focus_thread: RwSignal<Option<String>>, // a marker was clicked
```

with

```rust
pub struct Composer {
    pub thread_id: Option<String>,
    pub anchor: Option<Anchor>,
    pub model: String,
    pub effort: Effort,
}
```

`RunEvent`s apply to the matching run inside `comment_threads` in place, so
the running block updates without a reload. `CommentThreads` replaces the
whole list, like `Worktrees`.

`AppState` gains `agent_defaults: (String, Effort)` from config.

## UI

### Editor

The editor already tracks the cursor; the selection is read from the same
cursor, as start and end display lines, into `selection`. Display lines
map to head lines through the row table as the hover code does, and a
selection that lies wholly on removed lines has no anchor: `a` does nothing
and the `+` does not appear there.

The gutter gains, on hover over a line, a `+` at its left edge in the
accent colour, the way github.com's diff does. Clicking it opens the
composer for the current selection if the line is inside it, or for that
one line.

The gutter shows a small speech-bubble glyph on the first line of each
anchored thread, in the accent colour, or muted when outdated. Clicking
sets `focus_thread`.

### Thread column

A column on the right of the editor, resizable like the drawer, default
360 wide, hidden by default. Header: "Threads", a `+` for an unanchored
thread, and the close glyph.

Body, top to bottom: the open composer if any, threads for the open file,
a divider labelled "Elsewhere in this worktree", the rest. Each thread:

- Anchor line: `src/manifest.rs:41–58` in the meta size, clickable, opens
  and scrolls. "Worktree" for an unanchored thread. "Outdated" after it
  when set.
- Messages, in order. The user's in the body colour. The agent's rendered
  from markdown: paragraphs, inline code, fenced code in the code font,
  lists. Nothing more.
- Under an agent message, the run: steps collapsed to one line with a
  disclosure, expanded while running. The footer: `sonnet · medium · 14s ·
  $0.03`. While running, a Stop button in place of the footer.
- A failed run: the error in one line and a Retry that reopens the
  composer with the question filled.
- A follow-up composer at the bottom, collapsed to one line until clicked.
- A `...` at the top right with Delete.

The composer: the anchor line, a multi-line text box, and a row with the
model picker, the effort picker, and Send. Model and effort are popout
menus like the worktree menu. `Escape` closes an empty composer.

`focus_thread` scrolls the column to the thread and flashes its border.

### Keys

| Key | What it does |
|---|---|
| `a` | Open a composer for the selection, or for the worktree. |
| `Cmd-Enter` | Send the composer. |
| `Cmd-Shift-\` | Show or hide the thread column. |
| `Escape` | Close an empty composer, else as before. |

`a` stands down while any text box has focus, as `e` and `s` do.

## Harness

`perch --ask owner/repo#N path:start-end "question"`, beside `--dump-file`.
It opens the worktree, builds the prompt exactly as the app would, runs
it, and prints each event on its own line, then the answer. `--ask
owner/repo#N "question"` asks without an anchor. It is how the prompt and
the parser get checked without a window, and it exists before any UI does.

## Threading

Runs are child processes read on the backend runtime. Each run is a
spawned task owning the child and its stdout reader; events go over the
existing channel to the UI thread. Cancel sends a kill through a handle
kept in a map by run id. Nothing blocks the command loop.

## Build order

1. `perch-agent`: `RunSpec`, `RunEvent`, the line parser, the tool
   summariser, `Run::spawn` and `cancel` over `tokio::process`. Capture a
   dozen lines from `claude -p "say hello" --output-format stream-json
   --verbose` into a fixture and test the parser against them. Test the
   summariser on `Read`, `Grep`, `Bash`, and an unknown tool. Test that
   `cancel` ends the child and yields `Failed`. Commit.
2. `perch-github`: the three tables, the types, the cache methods. Tests:
   insert a thread with two messages and a run, load it back whole; steps
   round-trip; `fail_running_runs` touches only running rows; forgetting a
   worktree removes its threads. Commit.
3. `perch-app`: the prompt builder as a pure function from anchor, file,
   pull request, and question to the two strings. Tests: selection inside
   a hunk carries the removed lines; no anchor omits the file block;
   outdated adds its line; a 200-line body is trimmed. Commit.
4. `perch-app`: `--ask`. Config. Run it against a real pull request and
   read the answer. Commit.
5. Backend: commands, events, the run task, the outdated check, startup
   cleanup. Commit.
6. UI: thread column with threads rendering from the cache, the composer,
   the running block, model and effort pickers. Ask an unanchored question
   end to end. Commit.
7. UI: selection from the editor, the gutter `+` and markers, `a`,
   `focus_thread`, the outdated treatment. Commit.
8. README: a Threads section. What it does, what the agent may and may not
   do, where the data is, the config keys. Commit.

Steps 1 through 3 are independent of each other.

## Acceptance

- Open a pull request, drag across five lines in a changed file, press
  `a`, ask "why was this changed?", send. The column opens, tool lines
  appear within a few seconds, the answer follows, the footer shows model,
  effort, time, and cost. The gutter shows a marker on the first line.
- Ask a follow-up in the same thread. The agent's answer refers to the
  first exchange without Perch having resent it. The footer of each answer
  shows its own model and effort when they differ.
- Quit Perch, reopen, open the worktree from the Worktrees list. The
  thread is there, the marker is there, the column opens when the file is
  opened.
- Ask with nothing selected. The composer says the worktree's name; the
  answer arrives; the thread lists under "Elsewhere in this worktree" from
  any file.
- Ask something that would need an edit, "rename this function". The run
  finishes with a step line saying the edit was not allowed and an answer
  that says so or works around it. `git status` in the worktree is clean.
- Press Stop during a run. The block says Stopped; the steps so far remain;
  a follow-up can still be sent.
- Set `agent.binary` to a path that does not exist. Send. The run fails
  within a second with a message that names the key.
- Push a commit to the pull request that changes the anchored file, open
  again. The thread says Outdated and its marker is muted. A thread on an
  untouched file does not.
- Archive the worktree; restore it. The threads are still there. Forget
  it; the rows are gone.
- `perch --ask` prints the same events and answer the window shows.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Do not use the Agent SDK or any library for the run. The binary on
  `PATH`, a child process, JSON lines. Perch should work with whatever
  version of the CLI the user has.
- Do not pass `--dangerously-skip-permissions` or `bypassPermissions`.
  The allowed tool list is the sandbox.
- Do not resend the thread's history on a follow-up. `--resume` is the
  memory. The fallback for a thread with no finished run is the only case
  that folds history into the prompt.
- Do not stream tokens. Message-level events, one flag away from more.
- Do not put the run's steps in `comment_messages`. A step is not a
  message.
- Do not name a type `Thread`. That word is taken.
- Do not write a row with origin `github`. The column exists so a later
  milestone does not need a migration.
- Do not re-anchor. Outdated is a flag and a muted marker.
- Do not add a chat panel, a history browser, or a session list. A thread
  is a thread.
- Do not call the language server from the prompt builder. Leave the slot.
- Do not block `ReviewReady` on loading threads or on the outdated check.
