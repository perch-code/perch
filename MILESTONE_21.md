# Perch: Milestone 21, Opening a pull request

## Goal

Work on a branch in Perch ends at the edge of GitHub. When you think
it's ready for review, you go to a terminal, `git push`, open
github.com, find the compare page and write the description there. Then
you come back and wait for `attach_pull_requests` to notice on a later
poll. After this milestone Perch does all of that from the branch
itself.

- A branch of your own with commits past its base has a **Pull
  request** surface at the foot of Conversation, where a pull request
  has Checks and Merge. It unfolds into a form: title, description,
  base, and a Draft box.
- **Open pull request** pushes the branch and creates the pull request.
  The branch's row becomes that pull request's row in place, at once,
  with its threads, runs and marks.
- On a pull request of yours, `3 unpushed` gets a **Push** button, so
  the rounds of fixes after review don't send you back to a terminal.
- The description can be written by Claude or Codex from the branch's
  commits and diff, into the form, for you to edit and send.

## Non-goals

- Force-pushing, in any form. A push the remote refuses is reported,
  and fixing it is for a terminal.
- Letting an agent push or open the pull request. Milestone 18 gives a
  Work run no token and no route to GitHub, and that stands. Perch
  pushes, when you press the button.
- Shelling out to `gh`. Perch calls GitHub's API itself, and a GUI
  launch may not find `gh` on its `PATH`.
- Reviewers, assignees, labels and milestones on the new pull request.
  Follow-up once the form has earned more rows.
- **Ready for review** on a draft, and converting back to draft. Open
  as draft, flip it on github.com. Follow-up.
- Editing the title or description of a pull request after it exists.
- Pull requests from a branch of a `local/<name>` repository, which has
  no GitHub, or from someone else's pull request checkout, which has
  one already.
- Stacking: a base that is another unmerged branch of yours is allowed,
  since the base is any branch, but Perch does nothing to keep the
  stack in order.

## Behaviour

### Where it is

`merge::surface` draws Checks and Merge between the conversation and
its composer on a pull request. On a branch, the same place gets a
**Pull request** surface, in the same restrained box.

It shows when all of these hold:

- the item has no subject, a checkout on disk, and a GitHub
  repository (not `is_local`);
- the checkout's branch is not its base branch: Start work with New
  branch unticked checks out the base itself;
- the branch has at least one commit its base doesn't
  (`git rev-list --count <base>..HEAD` in the worktree).

Folded, it's one line:

    Pull request   3 commits ahead of main          [Open pull request…]

Clicking the button unfolds the form in place. `Escape` or Cancel folds
it. Folding keeps what was typed until Perch quits, per item, as the
new-work view keeps its message.

Nothing goes in the bar. It's full on a narrow window already, and the
branch's one bar action stays Review.

### The form

- **Title**, one line. Filled in from the one commit's subject when the
  branch has one commit, else from the item's title (its Rename, or the
  first line of the Start work message).
- **Description**, the composer's box with the destination fixed to
  GitHub, so `@` completes people and files as a GitHub composer does
  and the grip sizes it. Filled in from the repository's pull request
  template when the worktree has one, in GitHub's order
  (`.github/pull_request_template.md`, then the root, then `docs/`,
  case-insensitively). Otherwise from the one commit's body, or with
  several commits, their subjects as a list, oldest first.
- **Base**, a chip like Start work's Base: the root's remote branches
  with no network, or GitHub's for a repository without a root. It
  opens on the checkout's `base_ref` with its remote prefix taken off
  (`origin/main` is `main`). Changing it recounts the folded line's
  commits.
- **Draft**, a checkbox, off.
- **Write with Claude** (or Codex, the model menu's pick), see
  [The description by an agent](#the-description-by-an-agent).
- **Open pull request** and **Cancel**.

The fills are made once, when the form first unfolds for the item.
Text you've typed is never refilled underneath you.

Warnings sit over the buttons and don't stop anything:

- **Uncommitted changes**: `4 files have uncommitted changes. They
  won't be in the pull request.` Perch doesn't commit for you.

Refusals disable Open pull request and say why in the same place:

- **A Work run is running on the item**: it may commit halfway
  through the push. `Wait for the run to finish.`
- **HEAD is not the branch**: the worktree was switched to another
  branch, or detached, in a terminal. `The worktree is on <other>, not
  <branch>.`
- **Empty title.**
- **No token.**

### Opening

**Open pull request** runs three steps, and the button says which:

1. **Checking…** Ask GitHub, unconditionally, whether a pull request
   is already open on the branch (`find_pull_for_branch` with no
   etag). One opened on github.com and not yet attached is taken as it
   is, with a notice: `#123 was already open on this branch.` Nothing
   is pushed or created.
2. **Pushing…** Push the branch to the repository its pull request's
   head names, the remote `head_repo_of` resolves:
   - A checkout under a root, or cut in a root (`source`), pushes with
     `Git::plain()`: your credential helpers and SSH key, never Perch's
     token, with `BatchMode=yes` as `fetch_branch` does, so a key that
     wants a passphrase fails rather than hangs. It sets the upstream
     (`-u`), so a later `git push` in a terminal goes to the same place.
   - A checkout in Perch's own clone pushes to its `origin` with the
     token helper, as its fetches do. No upstream.
   - The refspec is `HEAD:refs/heads/<branch>`, never `+`. A branch
     already up to date is a no-op, so pressing again after a failed
     create doesn't push twice.
3. **Opening…** `POST /repos/{repo}/pulls` with `title`, `body`,
   `head` (`<head owner>:<branch>`), `base` and `draft`, where `repo`
   is the item's repository: the upstream, for a root filed under its
   `upstream` from a fork.

On success the response goes straight into `take_pull_request`, under
the one lock, as `attach_pull_requests` does, and the lookup's etag for
the branch is cleared so the next poll asks afresh. The row keeps its
place and becomes the pull request's: glyph, number, author. If the
item is open, it stays open and turns into the pull request, as it
would if a poll attached one while it was open: the bar gets the
GitHub button, Conversation fetches the discussion, and the surface at
the foot becomes Checks and Merge.

On failure the form stays unfolded with what was typed, and an error
line says what failed:

- **The push was refused** because the remote's branch has commits the
  worktree doesn't: `The remote's <branch> has commits this worktree
  doesn't. Pull or rebase in a terminal, then try again.` Git's own
  message goes in the log, not the form.
- **The push couldn't authenticate**: `Couldn't push with your git
  credentials. Push from a terminal, then Open pull request again.`
  After a push from the terminal, step 2 is a no-op and step 3 goes
  ahead.
- **GitHub refused the create**: GitHub's message. A 422 saying a pull
  request already exists runs step 1 again and attaches it. A draft
  refused on a plan without drafts says so, and unticking Draft is the
  fix.
- **Offline**: `Offline.` Nothing is attempted.

A push that succeeded stays pushed when the create fails. In a root,
the push moved the remote-tracking ref, so Delete already counts the
branch's commits as held.

### Push

A pull request of yours with a checkout says `3 unpushed` in the
summary's Branch row and the bylines when the worktree is ahead
(Milestone 20). When it's ahead only, the Branch row gets **Push**
beside the status. It pushes as step 2 does, to the same remote, with
no refspec change and no force.

After it, the divergence is counted again, from the remote-tracking
ref the push just moved, and the pull request is fetched, as when a
notification arrives about the open pull request. Submit review and
Merge come back once GitHub's head is the worktree's.

`2 behind PR` and `3 unpushed · 2 behind PR` get no button: a push
there would be refused or would need force. The status is the reason.

### The description by an agent

**Write with Claude** runs a question in the worktree under Ask's rules
(read only, no network, nothing reaches GitHub). Perch's prompt names
the base and gives the commit log and `git diff --stat
<base>...HEAD`, and asks for a title on the first line, a blank line,
and a description in GitHub Markdown, written for reviewers. The run
reads what else it wants from the worktree.

- The answer fills Title and Description. If either holds text you
  typed, Perch asks first: `Replace what you wrote?`
- The button reads **Writing…** with a stop, while it runs. The
  Agents dropdown lists the run. It's not a thread and draws no card on
  Conversation.
- A failed or empty answer leaves the fields alone and says why.
- Nothing is sent to GitHub until you press Open pull request.

## Model

- `AppState` gains `pull_request_forms: RwSignal<HashMap<String,
  PullRequestForm>>`, keyed by item, until Perch quits:
  `PullRequestForm { title, body, base, draft, filled: Filled,
  unfolded: bool }`, where `Filled` records which fields still hold
  Perch's own fill.
- The folded line's facts, `BranchReadiness { ahead_of_base: u32,
  dirty_files: u32, head_branch: Option<String> }`, come from the
  backend when the item opens and when its head moves, beside the
  divergence count.
- `RunOwner::Description { item_id }`, a third owner kind beside
  `Discussion` and `Review`.

## Backend

- `perch-git`: `Preparer::push_branch(worktree, remote, branch,
  set_upstream, git)` taking the plain or token `Git`, `BatchMode` for
  SSH, `--porcelain` so a rejection is told from an auth failure by
  its status line rather than by git's prose. `readiness(worktree,
  base, branch)` for `BranchReadiness`.
- `perch-github`: `create_pull_request(repo, head, base, title, body,
  draft)`, parsing the response with the parser `find_pull_for_branch`
  uses. `Pull requests: write`, which Submit review needs already.
- The commands `CreatePullRequest { item_id, title, body, base, draft }`
  (not `OpenPullRequest`, which opens a pull request by number),
  `PushBranch { item_id }`, and `WriteDescription { item_id, backend,
  model }` with its stop. Each reports progress and its outcome as an
  event to the item.
- `CreatePullRequest` checks the refusals again in the backend, since
  the form's view of them may be old: a Work run admitted since, a
  branch switched since.
- Schema 30: `runs.owner_kind` accepts `'description'`. The `CHECK`
  constraint means rebuilding `runs`, in the preserving, transactional
  way the earlier run migrations did.

## Harness

Headless tests in the style of the ones they sit beside, with a local
bare repository as the remote and GitHub's paths answered by the test
server as `attach_pull_requests`' tests do.

- The surface: shows on a branch with commits past its base; not on
  one at its base, one whose branch is the base, a `local/` repository,
  or an item with a subject.
- Fills: one commit gives its subject and body; several give the
  item's title and the subjects; a template wins over both; the base
  loses its remote prefix; typed text isn't refilled.
- Refusals and warnings: a running Work run, a switched branch and a
  detached HEAD refuse, in the form and in the backend; uncommitted
  changes warn and don't refuse.
- Opening: an existing pull request is attached without a push or a
  create. A clean run pushes `HEAD:refs/heads/<branch>`, sets the
  upstream in a root and not in Perch's clone, posts the right `head`
  for a fork root, and turns the row into the pull request with its
  threads. A second press after a failed create doesn't push again.
- Failures: a non-fast-forward rejection and an auth failure each give
  their message; neither forces; a 422 "already exists" attaches.
- The token never reaches a push in a root: the command's environment
  has no `PERCH_GIT_TOKEN`.
- Push: shown for ahead only, pushes, recounts the divergence to none,
  and asks for the pull request. Not shown when behind or diverged.
- The description: the run's answer splits into title and body; typed
  text asks before replacing; the run has no thread and no card; the
  migration keeps every run.

## Build order

1. `push_branch`, `readiness` and `create_pull_request`, with their
   tests. Commit.
2. `CreatePullRequest` in the backend: the refusals, the lookup, the
   push, the create, `take_pull_request`, the events. Commit.
3. The Pull request surface: folded line, form, fills, base chip,
   warnings, refusals, progress and errors, and the open item turning
   into the pull request. Commit.
4. Push on `N unpushed`. Commit.
5. Schema 30 and Write with Claude. Commit.
6. README: an **Opening a pull request** section after Starting work;
   Starting work's list of what Perch writes in a root gains the push
   and the upstream; Keeping work safe loses the line about a branch
   pushed from Perch's clone not being seen as pushed, which now holds
   only for a push from a terminal; Authentication names `Contents:
   write` for a push from Perch's clone and `Pull requests: write` for
   opening. Commit.

## Acceptance

- Start work on a repository with a root, let the Work run finish, and
  commit in a terminal. Conversation's foot reads `Pull request · 1
  commit ahead of main`. Unfold it: the title is the commit's subject.
  Press Open pull request. Within a few seconds the row is `#N`, the
  item is the pull request, and its Agents row still has the run. On
  github.com the pull request is there with the description as typed.
  `git -C <root> branch -vv` shows the branch tracking `origin`.
- Do the same on a repository with no root: the push uses Perch's
  token, and `git -C <root>` is nowhere involved.
- On a root cloned from a fork with `upstream` declared, the pull
  request opens against the upstream from `me:<branch>`.
- Open a pull request on github.com for another branch before a poll
  attaches it, then press Open pull request in Perch: it's attached,
  with the notice, and nothing is pushed.
- Push a commit to the branch from another clone, commit locally, and
  press Open pull request: the push is refused with the rebase message,
  and nothing on the remote changed.
- Use a root whose SSH key wants a passphrase: the auth message shows.
  Push from a terminal, press again: the pull request opens.
- Leave a file modified: the warning names the count, and the pull
  request opens without it.
- Start a Work run and unfold the form: Open pull request is disabled
  until it ends.
- Commit twice in the pull request's worktree. The Branch row says `2
  unpushed` with Push. Press it: the status clears, the checks start on
  GitHub, and Merge is offered again once they allow it.
- Press Write with Claude on a branch of several commits: the title and
  description fill in, and no card appears on Conversation. Type in the
  title, press it again: Perch asks before replacing.
- Upgrade a database with runs of both owner kinds: they're all there
  after schema 30.

## Things the implementer should not do

- Don't force-push, and don't offer to. No `+` refspec, no
  `--force-with-lease`.
- Don't use Perch's token for a push in a root or a worktree cut in
  one, and don't write it to any config.
- Don't give an agent a way to push or create the pull request, and
  don't send anything an agent wrote to GitHub without the button.
- Don't commit, stash, rebase or switch branches for the user to make
  a push possible.
- Don't call `gh`.
- Don't create the pull request before the push has succeeded, and
  don't retry a refused push.
- Don't run `cargo fmt --all`. Format only new files.
