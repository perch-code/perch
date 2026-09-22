# Perch: Milestone 15, Post to GitHub

## Goal

Let the reader answer a pull request, not only ask about it. Today every
composer in Perch sends to Claude or Codex. A GitHub comment on a line
offers "Ask about this", which quotes it into an agent thread, and the
agent's answer stays in Perch. Nothing the reader writes reaches the
people on the pull request except the review verdict in the title bar,
which posts a body and an event and nothing else.

After this milestone a composer has a destination, and GitHub is one of
them. A comment on a line becomes a draft in a pending review, the way
github.com works, and the Submit review form lists the drafts and posts
them with the verdict. A reply under a human's comment is a draft in the
same review. A comment at the foot of the conversation is an issue
comment and posts at once, as github.com does. An agent's answer gains
"Post to GitHub", which puts its text in a GitHub composer to edit and
send.

The design decision the milestone turns on is that a thread has one
shape and two audiences. GitHub's review threads and Perch's agent
threads become the same `CommentThread`, and a message says where it
went: to GitHub, or to an agent and so to no one else. One thread can
hold a reviewer's comment, the reader's reply to them, and the agent's
private take on both, and both panes draw it with the same card. The
private lane is drawn as private so the reader never wonders what a
colleague can see.

## Non-goals

- Editing or deleting a comment already on GitHub, the reader's own
  included. Resolving or unresolving a thread. Each is a later
  milestone with the same model.
- Reviewing from the conversation pane's composer. The review body is
  the Submit review form's, as today.
- GitHub's own pending review. Drafts live in Perch, not on GitHub, so
  they can be written offline and github.com does not show them. See
  Submission for what this costs.
- Posting an agent's answer without the reader reading it. There is no
  auto-post, no "post all findings", no bot account. Every message to
  GitHub is one the reader wrote or edited and sent.
- Mentions, autocomplete, emoji pickers, attachments, suggested changes.
  A GitHub composer is the composer Perch has, with markdown.
- Comments on the base side of a split, or on lines outside the diff.
  GitHub allows both and Perch's anchor allows the first; the gutter
  today marks the head side only, and stays so.
- Issues. The conversation pane shows issue comments for an item that is
  an issue, and this milestone posts to pull requests only. The issue
  comment path is the same endpoint and can be opened later.
- Notifications about the reader's own comments. Perch already sees them
  as anyone else's.

## Behaviour

### One thread type

A GitHub review thread on a file is a `CommentThread` with `origin:
GitHub`, its first comment and replies as `CommentMessage`s with
`Author::GitHubUser`, the same `Anchor` a local thread has, and
`github_id` set on the thread and every message. The `LineThread`
grouping, the `CardItem::GitHub` variant and `github_threads_of` go.
The editor, the gutter and the conversation pane draw one thread type
through `thread_card`, and `cards.rs` orders them by anchor as it
orders local ones today.

Ingestion is a cache step, not a UI one. When a discussion is fetched
or refreshed, its review comments are grouped by `in_reply_to_id`, each
root becomes a thread keyed by its comment id, and the rows are
replaced. A thread already in the cache with that `github_id` keeps its
Perch id and any local messages under it: an agent's answer asked
through "Ask about this" belongs to the GitHub thread now, not a
sibling. `outdated` and `resolved` come from GitHub. A comment on a
line the head no longer has anchors at `original_line` and is
outdated, which the card already says.

The reactions row, the "view on GitHub" link and the nested `diff_hunk`
block a GitHub card draws today move to `message_view`, shown on a
message whose author is a GitHub user, since a thread no longer has a
kind of its own.

### Destination

The composer's model picker gains GitHub as a destination beside the
agents. The picker reads `Claude · Opus · High`, `Codex · …`, or
`GitHub`. When GitHub is picked, the effort picker is hidden and the
send button reads `Add to review` in a thread on a line, or `Comment`
at the foot of the conversation. The last destination is remembered as
the model is, separately for the inline and conversation faces.

The default destination follows the reply. A new composer on a line
defaults to what the reader last used there. A follow-up follows the
thread's last message: under a message by a GitHub user, or under the
reader's own draft or posted comment, GitHub; under an agent, that
agent. A GitHub user's comment also offers `Reply` on hover, beside
`Ask about this`, which opens the composer under the thread on GitHub
whatever the thread ends in, so the reader can answer a reviewer after
asking an agent about them. The reader flips the destination with one
click, and flipping it keeps the text. The composer at the foot of the
conversation always opens on an agent; the remembered destination is
for composers on lines.

GitHub is offered on lines and threads of an open pull request only.
On a branch, or a pull request that is closed or merged, a draft could
never post, so the picker has no GitHub entry and a composer opens on
an agent whatever the face remembers.

A message to an agent behaves exactly as today: `backend.ask`, a run,
the answer under it. Sending it to GitHub instead makes a draft.

### Drafts

A draft is a `CommentMessage` with `origin: GitHub`, `Author::User`,
and no `github_id`. It is written to the cache the moment it is sent
and survives the item closing, Perch restarting, and the network being
gone. A draft under a thread that does not exist yet creates a local
thread with the anchor, origin `GitHub`, no `github_id`: a thread that
is on its way to GitHub.

The card draws a draft as the reader's message with a `Draft` pill in
the GitHub colour after the author, and an edit and a delete on hover,
which the agent messages have too. Editing a draft opens it in place as
the composer with its text; sending replaces it. Deleting a draft that
was the only message in a GitHub-bound thread deletes the thread. The
gutter marks a line with a draft in the same colour.

In the conversation pane a draft reads the same, in its thread, in time
order.

### The private lane

In a thread that has anything GitHub-bound, every message that is not,
the agent's answers, their runs, and the reader's questions to them,
is drawn inset by the gutter width with a hairline rule down its left in
the muted colour and `Perch only` in small caps at the top of each run of
them. A thread that is wholly private, as every thread is today, draws
nothing new: the rule is a difference, not a decoration.

A message to GitHub is never inset. A draft and a posted comment sit
flush with the GitHub comments they answer.

### Post to GitHub

An agent message gains `Post to GitHub` in its hover actions, beside
copy. It opens a composer under the thread's last message, destination
GitHub, with the answer's markdown as the text, the caret at the end.
The reader edits and sends, and it is a draft like any other. The
agent's message stays where it was, private. Nothing records that the
draft came from it.

On a findings thread the same action is on the finding's message.

### Submission

The Submit review form lists the drafts above the body: each as one
line, `src/x.rs:41` and the first sixty characters, with a delete on
hover. The button in the title bar reads `Submit review · 3` while
drafts exist, and the form's summary reads `3 comments` or `1 reply,
2 comments`. `Discard all` sits at the foot of the list and confirms.
With drafts and no body, `Comment` is allowed; `Approve` and `Request
changes` still need what they need today.

Submitting posts one review through `submit_pull_request_review`, which
grows a `comments` array: for each draft that opens a thread, its path,
line, side, start line when it has one, and body, against the review's
`commit_id`. Replies to threads that already exist on GitHub cannot
ride in that call, so each posts through `POST
/pulls/{n}/comments` with `in_reply_to` right after the review lands,
in order. A reply posted this way is not part of the review on GitHub's
side; it is a comment on the thread with the same timestamp, and a
reader on github.com sees no difference. This is the cost of local
drafts and it is accepted.

Submission is one command and one event. The backend posts the review,
then the replies, and answers with what GitHub returned. A reply that
fails after the review landed leaves the review up and the reply a
draft, and the form says which. The cache then refetches the
discussion, and ingestion matches each returned comment to its draft by
the review id and the anchor, sets `github_id` on the message and the
thread, and the `Draft` pill goes. A draft that cannot be matched stays
a draft and the form says so, rather than being dropped or posted
twice.

While the head is stale the form is unavailable as today, and the
button still shows the count. Drafts anchor to the commit they were
written against and are kept through a head move. The form, when it is
available again, lists a draft whose commit is not the head with
`written against a1b2c3` in the muted colour, and the reader decides.

### Issue comments

The composer at the foot of the conversation pane offers GitHub as a
destination too. Sending posts at once through a new `create_issue_comment`,
`POST /issues/{n}/comments`, with a confirm in the composer's own
footer, `Post to github.com/owner/repo/pull/12?`, that one click or
Enter accepts. There is no draft. The message shows as pending, with the
send button disabled, until the answer arrives, then as the reader's
comment with the returned id. On failure the text is back in the
composer with the error under it.

A reply to an issue comment or a review summary goes the same way, since
GitHub has no threads there: it posts as a new issue comment, and Perch
prefixes nothing. The reader quotes if they want to.

### What does not change

Asking an agent, follow-ups, runs, findings, the Agents panel, reactions,
merge, the review verdict's body and event, and the `Ask about this`
action on a GitHub comment, which now opens an agent composer under the
GitHub thread itself rather than beside it.

## Model

In `perch_github::comments`:

- `ThreadOrigin::GitHub` is constructed. `CommentThread.github_id:
  Option<u64>` and `CommentMessage.github_id: Option<u64>` are read and
  written; the columns exist.
- `CommentMessage` gains `is_draft()`: origin `GitHub`, author `User`,
  `github_id` none. And `is_private()`: origin `Local`.
- `CommentMessage` gains `html_url: Option<String>`, `reactions:
  Reactions`, and `diff_hunk: Option<String>` for a message from GitHub.
  Three new columns, one migration.
- `CommentThread.is_github_bound()`: origin `GitHub`, whether or not
  `github_id` is set.

In `review::model`:

- `Destination { Agent { backend, model, effort }, GitHub }`. `Composer`
  carries one in place of `backend`, `model` and `effort`, and a
  `editing: Option<String>` for the draft message it replaces.
- `ReviewState` gains `drafts: Memo<Vec<Draft>>` derived from the
  threads, where `Draft { thread_id, message_id, anchor:
  Option<Anchor>, in_reply_to: Option<u64>, body, commit_sha }`. The
  form, the button and the gutter read it.
- `last_destination: [RwSignal<Destination>; 2]`, one per face.
- `posting: RwSignal<Option<Posting>>`, the issue comment in flight:
  its body, the token its answer carries, and the opening of the
  composer that sent it, so a late answer settles that composer and not
  one opened since.
- `LineThread`, `CardItem::GitHub`, `CardId::GitHub` go.

In `review::timeline` a GitHub thread on a line is a `Card::Thread` like
a local one, ordered by its first message's time.

## Backend

- `Command::SaveDraft { item_id, thread_id: Option<String>, anchor,
  in_reply_to: Option<u64>, body, editing: Option<String> }` writes the
  message, and the thread when new, and emits the threads changed.
- `Command::DeleteDraft { item_id, message_id }`.
- `Command::SubmitReview` gains the drafts. The backend reads them from
  the cache itself, so a stale UI cannot post a draft the reader
  deleted. It posts, refetches, reconciles, and answers
  `Event::ReviewSubmitted { posted: usize, unmatched: Vec<String>,
  failed: Vec<(String, String)> }`.
- `Command::PostIssueComment { item_id, body, token }` and
  `Event::IssueCommentPosted { token, result }`.
- Ingestion lives in `perch_github::cache`, `ingest_review_threads`,
  called wherever a discussion fetched from GitHub is stored. Grouping
  the comments into threads, `comments::review_threads_of`, is pure and
  tested; the adoption and replacement of rows runs inside the caller's
  transaction and is tested through the cache. A copy of the discussion
  Perch changes by its own hand, a reaction or an issue comment it just
  posted, is kept with `rewrite_discussion`, which does not ingest: the
  copy can be behind GitHub, and is not what GitHub lists.

In `perch_github::client`:

- `submit_pull_request_review` takes `comments: &[ReviewCommentDraft]`,
  `{ path, line, side, start_line, start_side, body }`, serialised
  into the request. Empty is today's behaviour.
- `reply_to_review_comment(repo, number, comment_id, body) -> ReviewComment`.
- `create_issue_comment(repo, number, body) -> IssueComment`.

The README's permissions section already lists `Pull requests: write`,
which covers all three.

## Harness

The pure parts are tested where they live: grouping review comments
into threads, with a reply whose root is missing landing as its own
root; ingestion keeping a Perch id and its local messages across a
refresh; matching returned comments to drafts by review id and anchor,
with one unmatched; the default destination for each kind of follow-up;
the form's summary text. The editor's headless tests gain a draft under
a GitHub thread drawn with the pill, a private answer under it drawn
inset, and `a` on a selection with GitHub as the destination producing
a draft thread. `perch --dump-file` prints a draft as `[draft]` after
the author and a private message as `[perch]`.

The client's tests use the recorded-response harness the review submit
already uses, with one recording for a review with comments, one for a
reply, one for an issue comment.

## Build order

1. Ingest GitHub review threads as `CommentThread`. The three new
   message columns and the migration. Remove `LineThread` and the
   GitHub card path; `message_view` draws reactions, the link and the
   hunk. `Ask about this` opens under the thread. Nothing posts yet.
   Commit.
2. `Destination` in the composer, the picker entry, the remembered
   choice, the default per follow-up. `SaveDraft`, `DeleteDraft`, the
   `Draft` pill, edit and delete on hover, the gutter mark. Commit.
3. The private lane: the inset, the rule, `Perch only`. Commit.
4. The client's `comments` array and `reply_to_review_comment`. The
   form lists drafts, the count on the button, `Discard all`. Submission,
   reconciliation, the stale-commit note. Commit.
5. `create_issue_comment`, the conversation pane's GitHub destination,
   the confirm, the pending state. Commit.
6. `Post to GitHub` on agent and finding messages. Commit.
7. README: a Posting section saying what posts when, that drafts are
   local and github.com does not show them, and that agent answers never
   post unsent. Strike the line that says reviews do not comment on
   GitHub. Commit.

## Acceptance

Against a pull request with review comments from someone else:

- Open a file with a reviewer's comment on a line. It is a card in the
  thread style, with the reviewer's avatar, reactions and the GitHub
  link on the message. Click `Ask about this`; the composer opens under
  it; send; Claude's answer appears under the reviewer's comment, inset,
  with `Perch only` above it and a rule down its left.
- Press `a` on a selection, pick GitHub, type, `Add to review`. The
  card shows the reader's message with `Draft`. The gutter marks the
  line. The title bar reads `Submit review · 1`. Quit Perch and reopen:
  the draft is there.
- Hover the draft, edit it, send. The text changed and there is one
  draft. Delete it. The thread is gone and the count is gone.
- Hover the reviewer's comment, `Reply`. The destination is GitHub
  without clicking. Send. A draft, flush with the reviewer's comment,
  and the agent's answer above it still inset.
- Hover Claude's answer, `Post to GitHub`. The composer holds its text.
  Change a word, send. A draft. Claude's message is unchanged.
- Open Submit review. Both drafts are listed with their locations.
  Pick `Comment`, submit. The pills go; each comment keeps the reader's
  byline, with its time, link and reactions. Refresh github.com: two
  replies on the reviewer's thread, in the reader's name, and no
  review, since both drafts were replies and an empty review is not
  posted.
- Push a commit to the branch with a draft pending. The title bar shows
  the stale head and the count. Update. Open the form; the draft reads
  `written against` its commit. Submit anyway; it posts against the
  commit it was written on.
- Go offline. Write a draft. It saves. Open Submit review: unavailable.
  Come back online: available, the draft listed.
- At the foot of the conversation, pick GitHub, type, `Comment`. The
  confirm names the pull request. Accept. The message reads pending
  for a moment, then it is a comment with the reader's avatar. Refresh
  github.com: it is there. Pick Claude instead and send: an agent
  thread, as today, private.
- Reply under an issue comment with GitHub picked. It posts as a new
  issue comment, not threaded.
- Every message to an agent, in every thread, is inset in a thread with
  a GitHub message and not inset in a thread without one. Nothing an
  agent wrote is ever on github.com.

## Things the implementer should not do

- Do not keep two thread types. If `thread_card` cannot draw a GitHub
  message, teach it to; do not keep `LineThread` alive beside
  `CommentThread` for the transition.
- Do not use GitHub's pending review. Drafts are Perch's until the
  reader submits.
- Do not post replies inside the review call. GitHub's `comments` array
  does not take `in_reply_to`, and a reply forced in as a new thread on
  the same line is a second thread, which is worse than a reply outside
  the review.
- Do not post anything from a signal. The backend reads the drafts from
  the cache at submit time. The UI's list is a view of them.
- Do not drop a draft that failed to match after submit. Say so and keep
  it.
- Do not put a destination on the thread. It is on the message. A thread
  is where a message is, not who it is for.
- Do not draw the private lane in a thread with no GitHub message. The
  inset means "this is not on GitHub, though this thread is", and in a
  thread that is nowhere it means nothing.
- Do not mark an agent's message as the source of a draft, or draw a
  draft differently because it started as one. Once the reader sends
  it, it is theirs.
- Do not confirm a draft. Confirm an issue comment, which posts, and the
  review, which the form already does.
- Do not send an agent message to GitHub because the destination was
  GitHub last time in another thread. The default is per follow-up
  first and remembered per face second.
- Do not add a setting for any of it.
