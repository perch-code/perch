# Perch: Milestone 25, the Changescape tab

The design of the Changescape tab. It follows the skeleton of `design/MILESTONE_24.md`. Paths are relative to the Perch root. The line numbers were correct on Oct 7, 2026, before the work, and they drift.

## Goal

Changescape is a separate tool. It explains how a pull request changes the shape of a codebase. Today it is a Claude Code skill. The skill writes one JSON file per PR, `page.json`. A separate viewer draws the same file as HTML. This milestone lets Perch show that JSON as a third mode of an item, Changescape, next to Conversation and Code. The tab is named Changescape and not Map, because one section inside the page is already called Map.

The Changescape tab is a way into Perch. It is not a second code viewer. Every element on the Changescape tab opens its lines in Perch's own Code mode, with Perch's editor, scopes, and language servers. Every review thread on the Changescape tab opens in Perch's thread UI.

When no changescape exists and the skill is not installed, Perch must look and behave exactly as it does today. No pill, no empty state, no menu entry, no log noise.

## Non-goals

- No web view. Perch stays a native Floem app.
- No graph layout library. Changescape lays out every diagram and ships it as SVG.
- No analysis inside Perch. Perch reads files that Changescape writes. Perch never computes a Changescape fact.
- No editing of the changescape. Reader notes on a changescape are a later milestone.
- No changescape for an item that is not a pull request, and no changescape for a local Work run. Both are later milestones.
- No change to Changescape. If Perch needs something from the files, write it down under "Requests to Changescape" in your final report.

## Behaviour

### Where changescapes live

Changescape writes every output under one cache root. The default root is `~/.changescape/cache`. The environment variable `CHANGESCAPE_CACHE` overrides it, and so does the Perch configuration below.

One build of one PR at one head commit has its own folder:

```
~/.changescape/cache/<owner>/<repo>/pr-<number>/<head_sha>/
  status.json     state of the build
  page.json       facts and captions, the only file that Perch draws
```

`<owner>/<repo>` is `Item.repo_full_name`. `<number>` and `<head_sha>` come from `PullRequest` (`crates/perch-github/src/pulls.rs:32`). Other folders under `<owner>/<repo>/`, for example the repo atlas, belong to Changescape. Perch ignores them.

`status.json`:

```json
{ "schema": 2, "state": "done", "started": "2026-10-07T18:02:11Z", "finished": "2026-10-07T18:04:40Z",
  "stage": "narrated", "error": null, "skill_version": "2.0.0", "page_schema": 3 }
```

- `state` is `running`, `done`, or `failed`.
- `stage` is `facts` (tools ran, no captions yet) or `narrated` (captions written). A `done` build at stage `facts` is drawable. It shows facts with no captions.
- If `state` is `failed`, `error` holds a short message.
- If `status.json` is missing but `page.json` exists, treat the build as `done`.
- `page_schema` is the format version of the folder's `page.json`. If it is absent, the version is 2.

### Lookup

For the open PR item, Perch looks up its changescape with this rule:

1. If the folder for the current `head_sha` exists, use it. The changescape is current.
2. If that folder does not exist, look for other `<head_sha>` folders of the same PR. If one exists, use the one with the newest `finished` time. The changescape is stale.
3. Otherwise there is no changescape.

Perch still draws a stale changescape, under a one-line banner that names the older commit. The banner text is `Changescape of an earlier commit, <sha7>.`

If the skill is available, the banner ends with a Rebuild link. "Building a changescape" below describes the build.

### Page format versions

Perch draws page format versions from `MIN_PAGE_SCHEMA` to `MAX_PAGE_SCHEMA` (`crates/perch-changescape/src/lib.rs`). Both are 3 now. Perch reads the version from `page_schema` in `status.json`. A folder with a page but no `status.json` counts as version 2. Perch does not keep a reader for older versions: an older page is refused, and the reader rebuilds it.

- A finished build below the lowest version is `Lookup::Outdated`. The tab says "This Changescape was built by an older version. Rebuild it to see this PR." If Perch can build, the tab shows Rebuild.
- A finished build above the highest version is `Lookup::TooNew`. The tab says "This Changescape needs a newer Perch."
- In both cases the pill stays, so the reader finds the link.
- The gate applies to every build that the lookup considers. An earlier head's build of another version is never a `Stale` stand-in. A running build does not show it meanwhile.
- When the page loads, Perch reads `schema` from `page.json` before it parses the rest, and applies the same gate (`Error::Version`, then `Lookup::with_page_schema`). This catches a status that misstates the version.

Lookups are file system reads only. Do them on the backend, never on the UI thread. Perch has no file watcher, so poll. When an item opens, look up its changescape once. While a build of the open item is `running`, read `status.json` every two seconds. Do not poll items that are not open.

### The Changescape pill

`crates/perch-app/src/review/switch.rs:17` builds the Conversation and Code pills. Add a Changescape pill between them. If one of these two conditions is true, show the pill:

- The lookup found a changescape, current or stale.
- A build of this item is running.

In every other case the switch must be identical to today, including its width and the debug names of its children.

With a running build and no earlier changescape, the Changescape body shows the run's progress. Reuse `review/run_view.rs` (`run_header`, `doing_now`, `step_line`).

### The tour

The Changescape body is the page's tour: the optional top-level `tour` in `page.json` (the `tour` section of `skill/docs/page-data.md`, and `docs/design/tour.md` in the Changescape repo). Perch draws only the tour. It still parses the page's `sections` and `views`, because the schema requires them, but it draws no view as a section. The skill writes no `page.html`: the HTML viewer is a separate tool. A page without a tour, or with a tour that does not read, shows one line and Rebuild.

The tour is one scrolling column with three parts:

1. The cover: the PR title, the summary, the bar of lines to read, skim, and skip, at most three flags, and the contents. The contents list the chapters, or the stops when there is one chapter. A click scrolls to the chapter or the stop.
2. The chapters. A chapter has a title, an intro, and its spine, a call diagram, before its stops. A long chapter can have `sections`, named groups of consecutive stops. A section header, with its place among the sections, its title, and its intro, comes before the section's first stop. A run of skim stops never crosses a header. Stops in no section come after the last section, with no header.
3. The ending, "Where you stand": the stops that changed since their marks, the stops to come back to, the read stops with no mark, the bands of lines in no stop, and the page's checks and actions. A check that the tools add has `view` and `rows`, and it lists the first three of its row elements as links, then how many more.

A read stop shows open, as a card: its claim, Before, Now, and Watch with a link to what `watch_ref` names, its pictures, what it relies on, its asides, the review threads on its lines, and four buttons.

A relies entry is unchanged code that the stop's new lines depend on. It shows the element's title as a link, how the stop uses it (calls, implements, or matches), and what it promises: the narration's `what`, else the first sentence of its doc comment. Code opens the symbol's head lines in place, cut from the checkout at `code.file` as a hunk window is. The file loads only when Code opens. An aside is context from another view: its question, its one-line answer, and the first three elements of its view's rows as links. Done and Come back to this are toggles. Chat about this opens the composer in the stop. Next scrolls to the next read stop, or to the ending after the last one, and records nothing. Each run of skim stops folds into one group. In the group, each stop is a row with its title, its Now sentence in muted text, and its line count. A click opens the stop in place.

A picture of kind `hunk` draws its windows from the file as Code has it, with `cards::code_row`. The file loads under the Branch scope, because page lines count from the merge base, so the tour sets the item's scope to Branch when it opens. A window holds every display row from the first to the last whose head line is in its head range or whose base line is in its base range. A run of more than eight unchanged lines folds to three lines at each end. A click on a line opens Code at that head line, and a removed line opens the head line after it. A picture of kind `seq` draws as a diagram (see "Diagrams"). A picture of another kind is skipped.

Until `tour.narrated` is true, the cover says that the narration comes later.

Two sides frame the column. The outline on the left lists the start, each chapter with its stops, and the end. A section's title is a muted label over its first stop. A stop shows its number, or a check when it is done, a return arrow when the reader will come back to it, and a delta when it changed since its mark. A skim stop says skim. The code map on the right lists the files of `tour.map` by package, each with bars for its added and removed lines. A package opens on a click, and all packages are open when there are at most three, or at most 24 files. A file whose stops are all done has a check. A file in no stop is in italics.

Both sides light up the stop in view: the last stop whose top passed a third of the column. The outline lights up its row. The code map lights up the files the stop holds, and opens their packages. A click on the outline goes to that place. A click on a file goes to its first stop, and a file in no stop opens in Code. The code map hides when the tab is narrower than 900 pixels, and the outline hides when it is narrower than 640.

### Progress

The reader's marks are in `~/.perch/perch.db`, table `tour_progress`, keyed by (repo, number, stop id). Each mark keeps the stop's `h`, a hash of its changed lines without line numbers. A stop is Done, Come back to this, or unmarked. Scrolling past a stop records nothing. The commit bar shows how many read stops are done. The table is created with `CREATE TABLE IF NOT EXISTS` when the cache opens, and a table from before the hashes gets its `h` column then. The schema version does not change, so a released Perch at the same version can still open the database. The backend sends the marks with the lookup (`Found.marks`).

If a push changes a stop's lines, the stop's `h` on the new page differs from the stored one. The stop then shows as changed since you marked it: a line over its text says which mark it had, the outline shows a delta, and the stop does not count as done. A mark or a stop with no `h` holds as it is. A new mark on the stop stores the new `h` and clears the change.

### Chat about a stop

Chat about this opens a composer for a new thread with `about` set to `tour:<stop id>`, so only that stop draws it. The thread anchors to the stop's first head window. The composer's `context` holds the chapter, the stop's text, and its windows. The first prompt carries it in a block labeled "what the reader is looking at".

### Diagrams

A `seq` picture or a spine ships its own SVG and `hot` map. Draw it the way `review/picture.rs` draws pictures: `usvg::Tree::from_str`, then `cx.draw_svg`. For clicks, map the pointer into SVG coordinates, then find the smallest hot node whose bounding box holds the point, and open its element. Highlight the hovered node with a ring. A diagram up to 1.6 times wider than the column fits it. A wider one keeps its size and scrolls. Expand shows a diagram over the tour, fitted to the room, and Escape or Close puts it away. Changescape colors the SVG for a light background, so it sits on a white card in every theme.

### Elements and navigation

`page.json` has a top-level `elements` map. A click on a hot node opens its element:

1. If the scope is not `Scope::Branch`, set it to `Branch` with `backend.set_scope`.
2. Take the first ref with a `head` range, and open its file at its first head line.
3. A module opens its whole file.
4. An element only on the base side opens its file split, at the head line its own diff comes nearest to.

Code points back to the tour. If the caret in Code is on a head line that a stop's window holds, a strip over the editor names the stop and its chapter. Its link, "On the tour", switches to the Changescape tab and scrolls to the stop. `StopIndex` maps each path to the stops' head windows.

### Review threads on the tour

A stop shows the threads whose anchor lines a window of the stop holds, on the same side: a head anchor in a head range, a base anchor in a base range. They are Perch's comment threads, GitHub's and the reader's own, drawn as Conversation draws them. `page.json` can also carry GitHub's review comments. Perch does not draw them, except to link a `comment:<kind>:<id>` ref to the comment's page on GitHub.

### Views a ref names

A flag's `ref` or a stop's `watch_ref` can name a view, `view:<id>`, for example `view:related`. Its link opens a popover over the tour with the view's title and metric, and a row for each element that the view's table rows name. Each row shows the element's title and the first line of its description. Perch draws no table, fold, or facet: when the page loads, it keeps only the `el` of each row of each block, in row order and each once, and it drops a row whose element the page does not hold. A click outside, Close, or Escape puts the popover away.

A click on a row opens the element by its kind:

- `pr:<n>` and `issue:<n>` open that item in Perch, as Open does, in the page's repository.
- `commit:<sha>` opens on Code as the commit's own changes when the checkout has it, and else the element's page on GitHub.
- An element with lines opens in Code, as "Elements and navigation" says.
- Anything else with a `url`, such as a person, opens its page on GitHub.

### Building a changescape

If both of these conditions are true, Perch offers to build a changescape:

- `find_binary("claude")` (`crates/perch-agent/src/discover.rs:51`) finds Claude Code.
- The Changescape skill is installed. Look for `SKILL.md` with `name: changescape` in its front matter, in `~/.claude/skills/*/` or in `~/.claude/plugins/**/skills/*/`, at most four folders deep. Cache the answer for the session. After each request to build, look again.

If either one is missing, Perch shows no build action anywhere.

The skill must also write a tour. A skill from before the tour still reports page format 3 and skill version 2.0.0, so Perch reads its `schema/page.v3.schema.json`: a schema with no `tour` property means an old skill. A build with it writes a page with no tour over the one in the cache, so Perch does not start one. The tab shows "The installed Changescape skill writes no tour. Update the skill to build one." where Build, Rebuild, or Retry would be. A build that does not start leaves the build folder as it is.

If both are present, add "Build Changescape" to the item's existing action menu, in the place that holds actions like asking an agent. Pick the least visible fitting place. Do not add a pill or a button to the item header for it. After a build starts, the Changescape pill appears, as described above. On a stale changescape, the banner's Rebuild link starts the same build.

A build is a Claude Code run, started by `Run::spawn_changescape` (`crates/perch-agent/src/changescape.rs`). It runs outside the durable run queue and has no `RunPurpose`, so it is not in the item's run list. A build that quitting cuts short stays `running` until the next build. It differs from review runs in three ways:

1. It must not use `--safe-mode`, because safe mode turns off skills (`crates/perch-agent/src/review.rs:553`, `claude_isolation_args`). Write a new argument builder next to that one. Keep `--print`, `--strict-mcp-config`, `--no-session-persistence`, `--output-format stream-json --verbose`, `--permission-mode dontAsk`, and `--permission-prompts none`.
2. Allowed tools: `Read`, `Grep`, `Glob`, `Write` and `Edit` under the build folder only, `Bash(uv run *)`, `Bash(gh pr *)`, `Bash(gh issue *)`, `Bash(git *)`, `cd`, and the read-only tools Claude pipes output through. There is no `gh api`. Disallow everything else.
3. Sandbox configuration: start from `claude_settings` (`crates/perch-agent/src/lib.rs:185`). Allow writes to the cache root. Allow network to `api.github.com` and `github.com`, which `gh` and `git fetch` need. Allow the package index that `uv` uses (`pypi.org` and `files.pythonhosted.org`). Keep everything else denied.

The run runs in the cache root, and the clone that Perch keeps under `~/.perch/repos/` is an added directory. The sandbox refuses writes to the git internals of the folder a run runs in, and a bare clone is all internals. The skill fetches the PR head into the clone as `refs/changescape/pr/<n>`, so new refs stay out of the user's worktree.

The prompt is:

```
Use the changescape skill on pull request #<number> of <owner>/<repo>.
The repository is at <clone>.
Write the output to <cache root>/<owner>/<repo>/pr-<number>/<head_sha>/.
```

Show the run's progress in the Changescape body while it runs, and write its steps to `perch-build.log` in the build folder. When the run ends, read `status.json` again. Two results count as a failed build: the state `failed`, and a run that exits with no `page.json`. Show the error of a failed build in the Changescape body, with a Retry link.

The skill marks the build `done` at stage `facts` before the agent writes the narration, so the facts page draws about a minute before the narration arrives. The narration is the tour's text: the summary, the chapter intros, the stop titles and their Before, Now, and Watch, and the diagram captions. While a build that Perch started is still running and the page is at stage `facts`, the commit bar adds "Writing the narration…". Perch keeps polling until the stage is `narrated` or the run ends.

A step of the skill can mark the build `failed` in the middle of a run. For example, `assemble.py` does this when the narration breaks a rule of the tour, such as a title of more than 12 words. The agent then fixes `narration.json` and runs the step again. So while a build that Perch started still runs, a `failed` status is not the end (`lookup_during_run`). If the build folder has a page, the tab keeps drawing it. If it has no page, the tab shows the build's progress. Only the status that the run leaves when it ends counts as a failure. Perch cannot tell this for a build started outside Perch, because the status does not say whether the narration is still coming.

Only one build per PR runs at a time. A second "Build Changescape" while one is running looks the PR up again and starts nothing.

### Configuration

Add a `changescape` table to `Config` (`crates/perch-github/src/config.rs:9`), with `#[serde(default)]`:

```toml
[changescape]
enabled = true          # false hides every Changescape feature, even when changescapes exist
cache = "~/.changescape/cache"
```

The order for the cache root is `CHANGESCAPE_CACHE`, then `changescape.cache`, then the default.

## The page schema

The page format has a JSON Schema for each version. Perch keeps copies in `crates/perch-changescape/schema/`: `page.v3.schema.json` and `status.schema.json`. They come from `skill/schema/` in the Changescape repo. Do not edit the copies. When the format changes, copy them again.

The schema defines the page. The Rust types for it are generated, not written by hand:

- `crates/perch-changescape/src/generated/v3.rs` comes from `page.v3.schema.json`. The crate exposes it as `perch_changescape::v3`.
- `crates/perch-changescape/src/generated/status.rs` comes from `status.schema.json`.

To generate both files again after a format change, install cargo-typify 0.8.0 and run the script:

```sh
cargo install cargo-typify --version 0.8.0
crates/perch-changescape/schema/generate.sh
```

Each generated file starts with a `// @generated` line and the command that made it. Do not edit generated code by hand. `.gitattributes` marks `src/generated/` as `linguist-generated`, so GitHub folds the files in pull request diffs. Changescape treats any path under a `generated/` folder as generated.

When version 4 comes, generate `v4` and remove `v3` in the same change.

The generated types are strict, so Perch reads a page in two steps (`LoadedPage::read` in `src/page.rs`). It does not parse the views' blocks. It keeps only the element each table row names, for "Views a ref names". It parses `tour` on its own: one bad stop drops the tour, not the page, and `LoadedPage::tour_error` says why. Then it parses the rest of the page. If the rest does not parse, the tab shows the error.

The shared conformance fixtures are in `crates/perch-changescape/tests/fixtures/conformance/`, copied from `skill/schema/fixtures/` in the Changescape repo. `tests/conformance.rs` checks each case against its `expect.md`. The window tests in `review/changescape/tests.rs` draw them.

## Model

Put file types and lookup in a new crate, `crates/perch-changescape`. It has no UI dependencies.

- The page and status types are generated from the schema. See "The page schema" above. `LoadedPage` holds a `v3::Page` with the tour read on its own. `page.rs` has the tour's helpers: verdicts, runs of skim stops, reading order, windows, and diagrams.
- `fn lookup(root: &Path, repo: &str, number: u64, head_sha: &str) -> Lookup`, where `Lookup` is `Current(Build)`, `Stale(Build)`, `Running(Build, Option<Build>)` (with the last drawable earlier build), `Failed(Build)`, `Outdated(Build)`, `TooNew(Build)`, or `None`. `Build` holds the folder, the head SHA, and the status.
- `fn load(build: &Build) -> Result<Arc<LoadedPage>>`. Parse once per build and per `finished` time. Cache the result.
- `fn skill_available() -> Option<PathBuf>` returns the skill folder.
- `struct StopIndex` maps a path to the stops' sorted head windows, for the Code strip.

In `perch-app`:

- Add `ItemTab::Changescape` (`review/model.rs:148`). Add an arm to each `match` on `ItemTab`: `ui/mod.rs:527`, `:552`, `:1632`, and `review/tabs.rs:226`. Search for others.
- `ReviewState.changescape` holds what the backend found and the stop index. `changescape_marks`, `changescape_opened`, `changescape_focus`, and `changescape_expanded` hold the marks, the skim stops opened in place, the stop to scroll to, and the expanded diagram.
- The views are in `review/changescape/`: `tour.rs` (the cover, the chapters, the ending), `stop.rs` (stops, skim groups, marks, threads, chat), `side.rs` (the outline and the code map), `view.rs` (the popover of a view's elements), `hunk.rs` (code windows), `svg.rs` (diagrams), `navigate.rs` (from the tour into Code), `prose.rs` (text with code spans, and the small parts every view shares), and `strip.rs` (the Code strip).

## Backend

- Look up changescapes on item open, with the existing backend request pattern. Send the result to the UI as a signal update.
- Poll `status.json` every two seconds only while a build of the open item is `running`.
- The build run goes through `perch-agent`, but not through the run queue, so it is not in `AppState.agent_runs`.

## Harness

- `--changescape owner/repo#N` prints what Perch finds for that PR: the lookup result, and the tour's chapters and stops with their verdicts, line counts, and picture kinds. Add it next to `--dump-file` in `main.rs:66-99`. Use it in the acceptance steps.
- Fixtures: the conformance fixtures from the Changescape repo, in `crates/perch-changescape/tests/fixtures/conformance/`.
- UI tests use `floem::headless::{HeadlessHarness, TestRoot}` (`ui/mod.rs:1145`). Give every new view a `.debug_name(...)`: `"Changescape pill"`, `"Changescape tour"`, `"Changescape chapter <id>"`, `"Changescape stop <id>"`, `"Changescape skim row <id>"`, `"Changescape code line"`, `"Changescape commit bar"`, `"Changescape code strip"`.

## Acceptance

1. With no cache root and no skill, a headless test finds no view named `Changescape pill`. The children of the switch have the same debug names as before this milestone.
2. With case 12 for the open PR at its current head SHA, the Changescape pill shows. The tab opens on the tour: the cover, the chapter, and the ending. The read stop is open, and the skim stop is a row.
3. With case 12 at a different head SHA, the commit bar says that the tour is of an earlier commit.
4. A click on a line of a stop's code opens Code at that head line, under the Branch scope.
5. A click on a hot node in a diagram opens Code at that element.
6. A page with no tour, or with a tour that does not read, shows one line and Rebuild. A view block of an unknown shape does not stop the page from loading.
7. A review thread on a line inside a stop's window shows on that stop.
8. With the skill installed, "Build Changescape" on a real PR creates `status.json` with `state: running`. The Changescape pill appears at once. When the run ends, the Changescape tab draws the new page.
9. `enabled = false` hides the pill and the build action, even with a changescape in the cache.

## Things the implementer should not do

- Do not embed a web view, and do not add a graph layout crate.
- If no changescape exists, do not change what Conversation or Code do.
- Do not read the cache on the UI thread.
- Do not use the review snapshot for line numbers. Changescapes use the PR's merge base and head.
- Do not post anything to GitHub from the Changescape tab.
- Do not run Changescape tools directly from Perch. The skill owns its tools, and their paths can change.
