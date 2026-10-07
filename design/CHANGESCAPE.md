# Perch: Milestone 25, the Changescape tab

Handoff for an agent that works in the Perch repository (`~/workspace/perch`). It follows the skeleton of `design/MILESTONE_24.md`. If the number is still free, copy it to `design/MILESTONE_25.md` in Perch. All Perch paths below are relative to the Perch root, and they were correct on Oct 7, 2026. Make sure that each one is still correct before you edit it.

## Goal

Changescape is a separate tool. It explains how a pull request changes the shape of a codebase. Today it is a Claude Code skill. The skill writes one JSON file per PR, `page.json`, and a static HTML page. This milestone lets Perch show that JSON as a third mode of an item, Changescape, next to Conversation and Code. The tab is named Changescape and not Map, because one section inside the page is already called Map.

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
  page.html       the same page for a browser
```

`<owner>/<repo>` is `Item.repo_full_name`. `<number>` and `<head_sha>` come from `PullRequest` (`crates/perch-github/src/pulls.rs:32`). Other folders under `<owner>/<repo>/`, for example the repo atlas, belong to Changescape. Perch ignores them.

`status.json`:

```json
{ "schema": 2, "state": "done", "started": "2026-10-07T18:02:11Z", "finished": "2026-10-07T18:04:40Z",
  "stage": "narrated", "error": null, "skill_version": "2.0.0" }
```

- `state` is `running`, `done`, or `failed`.
- `stage` is `facts` (tools ran, no captions yet) or `narrated` (captions written). A `done` build at stage `facts` is drawable. It shows facts with no captions.
- If `state` is `failed`, `error` holds a short message.
- If `status.json` is missing but `page.json` exists, treat the build as `done`.

### Lookup

For the open PR item, Perch looks up its changescape with this rule:

1. If the folder for the current `head_sha` exists, use it. The changescape is current.
2. If that folder does not exist, look for other `<head_sha>` folders of the same PR. If one exists, use the one with the newest `finished` time. The changescape is stale.
3. Otherwise there is no changescape.

Perch still draws a stale changescape, under a one-line banner that names the older commit. The banner text is `Changescape of an earlier commit, <sha7>.`

If the skill is available, the banner ends with a Rebuild link. "Building a changescape" below describes the build.

Lookups are file system reads only. Do them on the backend, never on the UI thread. Perch has no file watcher, so poll. When an item opens, look up its changescape once. While a build of the open item is `running`, read `status.json` every two seconds. Do not poll items that are not open.

### The Changescape pill

`crates/perch-app/src/review/switch.rs:17` builds the Conversation and Code pills. Add a Changescape pill between them. If one of these two conditions is true, show the pill:

- The lookup found a changescape, current or stale.
- A build of this item is running.

In every other case the switch must be identical to today, including its width and the debug names of its children.

With a running build and no earlier changescape, the Changescape body shows the run's progress. Reuse `review/run_view.rs` (`run_header`, `doing_now`, `step_line`).

### The Changescape body

The Changescape body has two parts: a rail of sections on the left and the selected section on the right.

`page.json` has a fixed list of sections, in a fixed order. Their working ids are `overview`, `context`, `map`, `behavior`, `responsibilities`, `contracts`, `surroundings`, and `code`. Draw them in the order of the file, and do not hard-code the names. Labels come from the file.

- A section with no views is one muted line in the rail. The line shows its `reason`, for example "No CI or packaging changes." It is not clickable.
- The `code` section is not drawn. Its rail entry is a link that switches to the Code pill.
- A section with several views shows them one under the other, each with its title, its caption, and its blocks.
- On open, the Changescape tab selects Overview. Remember the selected section per item, the same way `Pane.tab` remembers the tab (`review/model.rs:159`).

### Blocks

Each view holds a list of blocks. The exact shapes are in `skill/docs/page-data.md` in the Changescape repo (`~/workspace/changescape`). That file is the source of truth. If this document and that file disagree, follow that file and note the difference in your report. Draw these block types natively:

| Type | How to draw it |
| --- | --- |
| `narration`, `text` | A paragraph. Backtick spans are code spans. An `el` on the block or a span makes it a link. |
| `summary` | A short list of rows. Each row has a title, a metric, and a line of text, and opens the named section or view. |
| `table` | A table. Column kinds are `text`, `mono`, `num`, `status`, `bar`, and `refs`. A cell or row with `el` is a link. A row with `dim` is muted. |
| `svg` | An SVG picture with clickable shapes. See below. |

Any other block type is drawn as one muted line: "This view is in the browser page." The line has a button that opens `page.html` in the default browser, with `open` from `std::process::Command`. Unknown fields in known blocks are ignored. Parsing must never fail on a new field or a new block type.

### SVG blocks

Changescape lays out diagrams (the city map, import graphs, sequence diagrams, class and schema diagrams) and ships each as one SVG string:

```json
{ "type": "svg", "svg": "<svg ...>...</svg>", "hot": { "n12": "mod:dblect.sql.patterns", "n13": "fn:dblect.sql.patterns.window" } }
```

`hot` maps an SVG node `id` to an element id. Draw the SVG the way `review/picture.rs:289-370` draws pictures: `usvg::Tree::from_str`, then `cx.draw_svg`. For clicks, map the pointer into SVG coordinates, then find the topmost node in `hot` whose bounding box holds the point. usvg 0.46 has `Tree::node_by_id` and `Node::abs_bounding_box`. Highlight the hovered hot node with a rectangle drawn over the picture. Do not edit the SVG tree.

Large diagrams must scroll and zoom. Fit the width on open. Changescape colors the SVG for a light background. If Perch is in a dark theme, draw the picture on a light card. Do not recolor it.

### Elements and navigation

`page.json` has a top-level `elements` map from element id to element. An element has a title `t`, a kind `k`, a status `st`, and `refs`. Each ref is a file path `f` with an inclusive line range on the base side, the head side, or both. Lines are 1-based. Base is the PR's merge base, and head is `meta.head.sha`.

A click on anything that carries an element id does this:

1. If Perch's scope for the item is not `Scope::Branch` (`crates/perch-git/src/scope.rs:39`), set it to `Branch` with `backend.set_scope`. Line numbers in `page.json` are relative to the whole branch.
2. Take the first ref with a `head` range. Call `ReviewState::go_to_file(path, Some((head_start - 1, 0)))` (`review/model.rs:3089`). This also switches to Code.
3. If no ref has a `head` range, the element exists only on the base side, for example a removed function. Perch has no API to open a base line. Show the ref's `src` excerpt in a popover over the Changescape tab instead. `src` rows are `[base_line, head_line, op, text]`. Add a "Show in diff" button. It opens the file in Code with `Layout::Split` (`review/model.rs:213`). It jumps to the first head line of the nearest hunk. A base-line jump is a later milestone.
4. If the changescape is stale, line numbers can be wrong. Open the file at the line anyway, and keep the stale banner.

Code points back to the Changescape tab. If the caret in Code is inside the head range of a Changescape element, show a strip over the editor. The strip is one line with the title and kind of the element. The strip has a link, "On the changescape", that switches to Map and selects the first section that shows the element. After the changescape loads, build an index from path to head ranges. Do not scan `page.json` on every caret move.

### Review threads on the Changescape tab

`Discussion.review_comments` (`crates/perch-github/src/discussion.rs:45`) has each comment's `path`, `line`, and `side`. For each element, count the open threads whose path matches a ref and whose line is in the ref's range on the same side. `Side::Right` is head, and `Side::Left` is base. Show the count as a small badge on table rows, on hot SVG nodes, and in the Code strip. A click on a badge opens the first thread with the existing thread navigation (`review/cards.rs:1461`, `path_link`).

Perch's thread data is the truth. `page.json` can also carry review comments for the browser page. Perch ignores them.

### Building a changescape

If both of these conditions are true, Perch offers to build a changescape:

- `find_binary("claude")` (`crates/perch-agent/src/discover.rs:51`) finds Claude Code.
- The Changescape skill is installed. Look for `SKILL.md` with `name: changescape` in its front matter, in `~/.claude/skills/*/` or in `~/.claude/plugins/**/skills/*/`, at most four folders deep. Cache the answer for the session. After each request to build, look again.

If either one is missing, Perch shows no build action anywhere.

If both are present, add "Build Changescape" to the item's existing action menu, in the place that holds actions like asking an agent. Pick the least visible fitting place. Do not add a pill or a button to the item header for it. After a build starts, the Changescape pill appears, as described above. On a stale changescape, the banner's Rebuild link starts the same build.

A build is a Claude Code run with a new purpose, `RunPurpose::Changescape` (`crates/perch-github/src/comments.rs:512`). It differs from review runs in three ways:

1. It must not use `--safe-mode`, because safe mode turns off skills (`crates/perch-agent/src/review.rs:553`, `claude_isolation_args`). Write a new argument builder next to that one. Keep `--print`, `--strict-mcp-config`, `--no-session-persistence`, `--output-format stream-json --verbose`, `--permission-mode dontAsk`, and `--permission-prompts none`.
2. Allowed tools: `Read`, `Grep`, `Glob`, `Write` and `Edit` under the build folder only, `Bash(uv run *)`, `Bash(gh pr *)`, `Bash(gh api *)`, `Bash(gh issue *)`, and `Bash(git *)`. Disallow everything else.
3. Sandbox configuration: start from `claude_settings` (`crates/perch-agent/src/lib.rs:185`). Allow writes to the cache root. Allow network to `api.github.com` and `github.com`, which `gh` and `git fetch` need. Allow the package index that `uv` uses (`pypi.org` and `files.pythonhosted.org`). Keep everything else denied.

Run it in the clone that Perch keeps under `~/.perch/repos/`, not in the user's worktree. The skill fetches the PR head into `refs/changescape/pr/<n>`. It never touches the work tree. A run in the clone also keeps new refs out of the user's worktree.

The prompt is:

```
Use the changescape skill on pull request #<number> of <owner>/<repo>, in this repository.
Write the output to <cache root>/<owner>/<repo>/pr-<number>/<head_sha>/.
```

Show the run's events in the Changescape body while it runs, and in the item's existing run list. When the run ends, read `status.json` again. Two results count as a failed build: the state `failed`, and a run that exits with no `page.json`. Show the error of a failed build in the Changescape body, with a Retry link.

Only one build per PR runs at a time. A second "Build Changescape" while one is running selects the Changescape pill.

### Configuration

Add a `changescape` table to `Config` (`crates/perch-github/src/config.rs:9`), with `#[serde(default)]`:

```toml
[changescape]
enabled = true          # false hides every Changescape feature, even when changescapes exist
cache = "~/.changescape/cache"
```

The order for the cache root is `CHANGESCAPE_CACHE`, then `changescape.cache`, then the default.

## Model

Put file types and lookup in a new crate, `crates/perch-changescape`. It has no UI dependencies.

- `Page`, `Section`, `View`, `Block`, `Element`, `Ref`, and `Status` are serde types. `Block` is an enum tagged by `type`, with an `Other` variant for unknown types (`#[serde(other)]` or a custom deserializer). Every struct ignores unknown fields.
- `fn lookup(root: &Path, repo: &str, number: u64, head_sha: &str) -> Lookup`, where `Lookup` is `Current(Build)`, `Stale(Build)`, `Running(Build)`, or `None`. `Build` holds the folder, the head SHA, and the status.
- `fn load(build: &Build) -> Result<Page>`. Parse once per build and per `finished` time. Cache the result.
- `fn skill_available() -> Option<PathBuf>` returns the skill folder.
- `struct LineIndex` maps a path to sorted head ranges with element ids, for the Code strip.

In `perch-app`:

- Add `ItemTab::Changescape` (`review/model.rs:148`). Add an arm to each `match` on `ItemTab`: `ui/mod.rs:527`, `:552`, `:1632`, and `review/tabs.rs:226`. Search for others.
- Add `ReviewState.changescape: RwSignal<Option<ChangescapeState>>`, which holds the `Lookup`, the loaded `Page`, the selected section, and the line index.
- Add the Changescape body in `review/mod.rs:196`, next to `showing_code()` and `item_view()`. Put the views in a new module, `review/changescape/`, with one file for the rail, one for blocks, and one for SVG.

## Backend

- Look up changescapes on item open, with the existing backend request pattern. Send the result to the UI as a signal update.
- Poll `status.json` every two seconds only while a build of the open item is `running`.
- The build run goes through `perch-agent` like other runs, so it appears in `AppState.agent_runs` (`state.rs:288-329`).

## Harness

- `--changescape owner/repo#N` prints what Perch finds for that PR: the lookup result, the sections with their view counts, and the block types with counts. Add it next to `--dump-file` in `main.rs:66-99`. Use it in the acceptance steps.
- Fixtures: run the skill once on `dvryaboy/dblect` PR 356, and copy the build folder into `crates/perch-changescape/tests/fixtures/`. Add a second fixture that has an unknown block type and unknown fields.
- UI tests use `floem::headless::{HeadlessHarness, TestRoot}` (`ui/mod.rs:1145`). Give every new view a `.debug_name(...)`: `"Changescape pill"`, `"Changescape rail"`, `"Changescape section <id>"`, `"Changescape block <type>"`, `"Changescape stale banner"`, `"Changescape code strip"`.

## Build order

1. `perch-changescape`: types, lookup, load, and tests on the fixtures.
2. Configuration and the `--changescape` command.
3. `ItemTab::Changescape`, the pill, and the rail with empty sections. The pill shows only with a fixture changescape in a temporary cache root.
4. The `narration`, `text`, `summary`, and `table` blocks, and the fallback line.
5. Element clicks into Code, with the scope switch and the base-only popover.
6. The `svg` block with hit testing, hover, scroll, and zoom.
7. Thread badges and the Code strip.
8. Skill detection, the build run, progress, and errors.

Each step must build and pass tests before the next one starts.

## Acceptance

1. With no cache root and no skill, a headless test finds no view named `Changescape pill`. The children of the switch have the same debug names as before this milestone.
2. With a fixture changescape for the open PR at its current head SHA, the Changescape pill shows. Overview is selected, and every section in the file appears in the rail in file order.
3. With the fixture at a different head SHA, the stale banner shows.
4. A click on a table row with a function element opens Code at the first line of the function, at head.
5. A click on a removed function shows the popover with its base excerpt.
6. A click on a hot node in an SVG block opens Code at that element.
7. An unknown block type draws the fallback line, and the page still loads.
8. A review comment on a line inside an element's head range shows a badge with count 1 on that element.
9. With the skill installed, "Build Changescape" on a real PR creates `status.json` with `state: running`. The Changescape pill appears at once. When the run ends, the Changescape tab draws the new page.
10. `enabled = false` hides the pill and the build action, even with a changescape in the cache.

## Requests to Changescape

The Changescape side will provide these. If one is missing when you start, use the fixtures, and list what you missed in your report.

- A default output folder at the cache path above, with `status.json` and `page.html` next to `page.json`.
- `schema: 2` at the top of `page.json` and in `status.json`.
- `sections` in `page.json` in display order. Each section has its label and its view ids. An empty section also has a `reason`.
- `svg` blocks with `hot` maps for every diagram.

## Things the implementer should not do

- Do not embed a web view, and do not add a graph layout crate.
- If no changescape exists, do not change what Conversation or Code do.
- Do not read the cache on the UI thread.
- Do not use the review snapshot for line numbers. Changescapes use the PR's merge base and head.
- Do not post anything to GitHub from the Changescape tab.
- Do not run Changescape tools directly from Perch. The skill owns its tools, and their paths can change.
