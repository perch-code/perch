# Perch: Milestone 24, Language servers beyond Rust

## Goal

Code navigation is Rust only. Open a Go, TypeScript or Java pull request
and the editor highlights nothing past plain text, and hover, go to
definition, references and the call hierarchy do nothing. The client in
`perch-lsp` is generic; what isn't is everything around it: `find_server`
knows one server, a window holds one, and the status says
`rust-analyzer` whatever is running.

And when the one server Perch knows is missing, Perch guesses. A checkout
that pins Rust 1.98.1 without its rust-analyzer component falls back to
rustup's proxy, which exits, or to Homebrew's rust-analyzer, which can't
read a newer cargo's metadata. Either way the user gets an error that
names the symptom and not the fix.

After this milestone:

- Go, TypeScript, JavaScript and Java files are highlighted, and get the
  navigation Rust files have, from gopls, typescript-language-server and
  jdtls.
- A worktree runs one server per kind it needs, started when its files
  show up, and each request goes to the server for its file.
- A server that isn't installed is named, with what it needs and an
  **Install** button that runs one shown command.
- Perch installs the servers that don't belong to a toolchain into
  `~/.perch/servers`, pinned to versions Perch's source names, and keeps
  that directory tidy.
- rust-analyzer comes from the checkout's own toolchain or not at all when
  rustup manages it, and Install adds the component to that toolchain.

## Non-goals

- Other languages: Python, C and C++, Ruby, Kotlin, Swift. Each is an
  entry in the registry this milestone adds, not part of it.
- Installing a project's dependencies. Perch never runs `npm install`,
  `mvn` or `go mod download` in a checkout. What a server fetches through
  the project's own build tool (gopls through `go list`, jdtls through
  Maven or Gradle) is that tool's business, under the user's settings for
  it (`GOPROXY`, `GOTOOLCHAIN`, `~/.m2`, `~/.gradle`).
- Installing a language: Go, Node or a JDK. Perch says which is missing.
  The one exception is a rustup toolchain a checkout pins, since its
  rust-analyzer only comes with it.
- A mode that runs no project code for outside contributors' pull
  requests. rust-analyzer already runs build scripts and proc macros, and
  jdtls's Gradle import runs `build.gradle`. That's its own milestone.
- Settings. Servers are found or installed, not configured, as Milestone 2
  said of rust-analyzer. No per-repository server choice.
- Updating anything without a click, on a schedule or in the background.
- Two servers for one language, such as ESLint beside the TypeScript
  server.
- Drawing diagnostics. They're still collected and not drawn.
- The base side of a split, which no server sees.

## Behaviour

### Languages and servers

| Language | Files | LSP language id | Server | Project markers |
| --- | --- | --- | --- | --- |
| Rust | `.rs` | `rust` | rust-analyzer | `Cargo.toml` |
| Go | `.go` | `go` | gopls | `go.mod`, `go.work` |
| TypeScript | `.ts` `.mts` `.cts`, `.tsx` | `typescript`, `typescriptreact` | typescript-language-server | `tsconfig.json`, `package.json` |
| JavaScript | `.js` `.mjs` `.cjs`, `.jsx` | `javascript`, `javascriptreact` | typescript-language-server | `jsconfig.json`, `package.json` |
| Java | `.java` | `java` | jdtls | `pom.xml`, `build.gradle(.kts)`, `settings.gradle(.kts)` |

TypeScript and JavaScript share one server process. A worktree has at most
one server of each kind, so four at most, not five.

`perch-syntax` gains grammars for Go, JavaScript (with JSX), TypeScript,
TSX and Java, at versions that build against its tree-sitter 0.27, and
`Language::from_fence` for the names a fence gives them. Fenced blocks
aren't coloured anywhere yet, Rust's included; what a fence named `go`,
`ts`, `typescript`, `tsx`, `js`, `javascript`, `jsx` or `java` gets is
its names looked up, as a `rust` fence's are. `theme::language_color`
gains a colour for each.

### When a server starts

Today rust-analyzer starts whenever an item with a checkout opens. Now:

- **At open**, a server for each language among the pull request's
  changed files. An item with none of a language that has a server (an
  issue, local work with nothing changed yet, a pull request that only
  touches docs) starts one for each project marker at the worktree's
  root instead, so names in its prose can still be looked up.
- **On opening a file** whose language has a server and whose worktree
  has none of that kind running, it starts then. A definition, a
  reference or a search hit opens files too, and counts.
- Every server is rooted at the worktree, as rust-analyzer is. A `go.mod`
  or `package.json` in a subdirectory is the server's to find: gopls,
  tsserver and jdtls all handle nested projects from the root.
- Leaving an item sets its servers aside as `kept_servers` does today,
  each keyed by window, worktree and binary. `KEPT` counts servers, so two
  kept servers might be one worktree's Go and TypeScript.

### Which server answers

- A hover, goto, references or call hierarchy request goes to the server
  for the file it's about. A file with no server kind gets nothing, as a
  Markdown file does today.
- A name looked up from prose (a description's names, `code_refs`) asks
  every usable server of the worktree and merges what they find. A name
  inside a fenced block asks only the server for the fence's language,
  when it names one with a server.
- A goto that can't be asked says which server and why: ``No definition
  found for `Handler` (gopls indexing)``.
- Statuses and warnings name their server: `gopls indexing 42%`,
  `typescript-language-server failed: …`. A failed or unhealthy server
  still gets one error toast when its status changes, as rust-analyzer's
  does. One server failing leaves the others usable.

### Health

rust-analyzer says it's unwell with `experimental/serverStatus`. The
others don't, so:

- `window/showMessage` of type Error makes the server `Unhealthy` with its
  message. Type Warning is a `ServerEvent::Warning`, told once per
  message, as rust-analyzer's warnings are. rust-analyzer's own messages
  leave its status alone: its server status already says.
- jdtls's `language/status` maps as: `Starting` to `Indexing`, `Started`
  and `ServiceReady` to `Ready`, and `Error` to `Unhealthy`.
  `ProjectStatus` says `OK` once a project has imported, which is no
  news.
- `$/progress` still gives `Indexing { percent }` for every server. gopls
  and jdtls report it. typescript-language-server mostly doesn't, and is
  `Ready` once initialized.

### Finding a server

For each kind, the first of these that exists:

1. **The project's own.** For rust-analyzer, the checkout toolchain's,
   from `rustup which rust-analyzer` with `RUSTUP_AUTO_INSTALL=0`, as now.
   The other kinds have no per-project binary. The TypeScript server is
   told to use the checkout's `node_modules/typescript/lib` (see
   [TypeScript and JavaScript](#typescript-and-javascript)), whichever
   binary runs it.
2. **The user's**, on `PATH` (with the install dirs `main` adds).
3. **Perch's own**, the newest version under `~/.perch/servers/<server>/`.

Two rules for Rust:

- rustup's proxy isn't a candidate. `~/.cargo/bin/rust-analyzer`, or any
  `rust-analyzer` on `PATH` that is the same file as `rustup`, only ever
  runs the toolchain's rust-analyzer or exits.
- When rustup manages the checkout's toolchain and that toolchain has no
  rust-analyzer, the server is missing. Perch doesn't fall back to another
  release's rust-analyzer: one from a different release fails on a newer
  cargo's metadata with `Failed to load workspaces`, which tells the user
  nothing about the fix. Without rustup, `PATH` as today.

`rustup which` tells the two Rust cases apart (rustup 1.29):

- `error: unknown binary 'rust-analyzer' in toolchain '<t>'`: the
  toolchain is installed without the component.
- `error: toolchain '<t>' is not installed`: the toolchain is missing too.

Take `<t>` from those lines and nothing else from their prose. Any other
failure of `rustup which` is treated as no rustup.

### Missing servers

A server that isn't found gets a toast that stays until it's closed:

- `rust-analyzer isn't installed for Rust 1.98.1.` **Install**
- `gopls isn't installed.` **Install**
- `gopls needs Go, which isn't on your PATH.` No Install. The same for
  Node (`npm` too) and for a JDK 21 or later.

The command Install runs is the toast's tooltip, word for word. A closed
toast stays closed for that server until Perch restarts. A goto that
can't be asked because the server is missing says so in its notice and
offers Install there too.

| Server | Needs | Install runs | Into |
| --- | --- | --- | --- |
| rust-analyzer | rustup | `rustup component add rust-analyzer --toolchain <t>`, or `rustup toolchain install <t> --profile minimal --component rust-analyzer` when the toolchain is missing | the toolchain |
| gopls | `go` | `go install golang.org/x/tools/gopls@v<pin>` with `GOBIN` set to Perch's directory and `GOTOOLCHAIN=local` | `~/.perch/servers/gopls/<pin>/` |
| typescript-language-server | `node`, `npm` | `npm install --prefix <dir> --ignore-scripts --no-fund --no-audit typescript-language-server@<pin> typescript@<pin>` | `~/.perch/servers/typescript-language-server/<pin>/` |
| jdtls | a JDK 21+ | a download of the release tarball from `download.eclipse.org`, checked against the pinned SHA-256, then unpacked | `~/.perch/servers/jdtls/<pin>/` |

Pins are constants in `perch-lsp`, a version per server and a checksum
for jdtls. Bumping one is a commit, like bumping a dependency.

### Installing

- The backend runs the install with a 10-minute timeout. Its output goes
  to the log. The toast becomes `Installing gopls…` with no buttons until
  it ends. (Perch has no spinner to give it.)
- Perch installs into `<pin>.partial` and renames it to `<pin>` when the
  install succeeds, so a crash or a quit never leaves half a server where
  Perch looks. `.partial` directories are removed at startup.
- On success the server starts for every window whose open worktree wants
  that kind, and the toast goes. Rust's install starts only the windows on
  a checkout of that toolchain.
- On failure the toast is an error with the command's last `error` line,
  or its last line, as an exiting server's is.
- One install per server at a time. A second Install, from another window
  or a notice, joins the one running.
- Perch never writes outside `~/.perch/servers`, except through rustup.
  No `npm -g`, no `~/go/bin`, no Homebrew.

### Managing

- When the newest of Perch's installs of a server is older than the pin,
  the server's first start in a session also gets a toast:
  `gopls 0.20.0 is available.` **Update** (the install again, at the pin).
  The old version keeps working meanwhile.
- After an install, older versions of that server under
  `~/.perch/servers` are removed at the next startup, when nothing runs
  them.
- Perch never removes or updates a server it didn't install. rust-analyzer
  components are rustup's to manage.
- Removing `~/.perch/servers` uninstalls everything Perch installed.

### Rust

Beyond the two rules in [Finding a server](#finding-a-server), unchanged:
same options, same external-file labels.

A checkout whose toolchain lacks the component, and that worked until now
through Homebrew's rust-analyzer, gets the Install toast instead. That's
deliberate: one click, and from then on the server is the toolchain's.

### Go

- `gopls`, no arguments (it serves stdio by default), no initialization
  options.
- Found on `PATH`, else in Go's default install directory, `~/go/bin`,
  which a Finder launch has no `PATH` to, else Perch's own.
- The pin depends on the local Go, from `go version` run with
  `GOTOOLCHAIN=local` outside any module: gopls 0.23.0 builds only with Go
  1.26, so Go 1.25 gets 0.21.1. The install runs with `GOTOOLCHAIN=local`
  too, so the go command never downloads a newer Go for it; a Go too old
  for every pin fails the install with Go's own line saying which it
  needs.
- External files from the module cache, `…/pkg/mod/<module>@<version>/…`,
  are labelled `<module> <version>`, with the cache's `!x` escapes turned
  back into uppercase. Files under `go env GOROOT`'s `src/`, or a
  downloaded toolchain's (`golang.org/toolchain@…/src/`), are labelled
  `go <package>`, e.g. `go net/http`.

### TypeScript and JavaScript

- `typescript-language-server --stdio`.
- Initialization options: `tsserver.path` is the checkout's
  `node_modules/typescript/lib` when it has a `tsserver.js`, so the
  project's own TypeScript answers. TypeScript 7, the native port, has
  none, so a checkout on it falls back like one without TypeScript: to the
  `typescript` installed beside the server (Perch's own) or, for a `PATH`
  server, unset. Perch pins typescript-language-server 6.0.1 and
  TypeScript 6.0.3, the last with a tsserver.
- The warning is told once as the server starts, from `start_warnings`.
- A checkout with a `package.json` at its root and no `node_modules` gets
  one warning: `No node_modules in this checkout, so imports from packages
  won't resolve.` Perch doesn't install them, since that runs the PR's
  install scripts. A worktree under the user's roots usually has them.
- External files: the package name after the last `node_modules/`,
  including its `@scope/`, and the version too for pnpm's
  `.pnpm/<name>@<version>/`. TypeScript's own `lib.*.d.ts` files are
  labelled `typescript`.

### Java

- **The JDK**: `/usr/libexec/java_home -v 21+`, else `JAVA_HOME` if its
  `java -version` is 21 or later, else none. jdtls needs 21 to run, but a
  project can target any older Java.
- **Launch**: a `jdtls` on `PATH` (Homebrew's wrapper) is run with
  `--jvm-arg=-Djava.import.generatesMetadataFilesAtProjectRoot=false -data
  <dir>`. Perch's own is run directly: `java
  -Djava.import.generatesMetadataFilesAtProjectRoot=false
  -Declipse.application=org.eclipse.jdt.ls.core.id1
  -Dosgi.bundles.defaultStartLevel=4
  -Declipse.product=org.eclipse.jdt.ls.core.product -Xmx2G
  --add-modules=ALL-SYSTEM --add-opens java.base/java.util=ALL-UNNAMED
  --add-opens java.base/java.lang=ALL-UNNAMED -jar
  plugins/org.eclipse.equinox.launcher_*.jar -configuration
  config_mac_arm` (`config_mac` on Intel) `-data <dir>`.
- **Data**: `<dir>` is `~/.perch/servers/jdtls-data/<first 12 hex of the
  worktree path's SHA-256>`. Never inside the worktree: it would count as
  untracked files and keep Perch's worktree cleanup from removing it.
  Perch removes a worktree's data directory with the worktree, and at
  startup any whose worktree is gone.
- **Initialization options**: `extendedClientCapabilities.
  classFileContentsSupport: true`, so definitions in a jar come back as
  `jdt://` URIs rather than nothing; and `java.autobuild.enabled: false`.
- **Nothing in the checkout**: tried on a Maven project, jdtls wrote
  `.project`, `.classpath` and `.settings/` into it, and its build wrote
  `target/classes`. The metadata setting has to be the JVM property
  above, since jdtls's file system reads it as it starts; the build is
  turned off, since navigating needs no classes.
- **Startup**: jdtls can take longer than the five seconds the handshake
  waits today. `ServerSpec` gains an initialize timeout: 60 seconds for
  jdtls, five for the rest. The import after it can take minutes on a
  large Gradle build, shown as indexing.
- **Class files**: a location whose URI is `jdt://…` opens as an external,
  read-only tab with the text from `java/classFileContents` for that URI.
  The tab is keyed by the URI without its query (which names the project
  and its classpath, slashes and all, and would make the tab's name
  unreadable); the client keeps the whole URI for it, and hover and goto
  inside it go to jdtls with that. It's labelled with the jar's name
  (`guava-33.3.1-jre.jar`), or `JDK` for the platform's classes, which
  jdtls names by module (`java.base`). With sources attached, jdtls names
  the file `List.java` rather than `List.class`.

## Model

- `ReviewState.lsp_status: RwSignal<Status>` becomes
  `RwSignal<BTreeMap<ServerKind, Status>>`, with
  `ReviewState::server_status(path) -> Option<Status>` for the file's
  kind. The callers that ask `is_usable()` (`sticky.rs`, `code_refs.rs`,
  `references.rs`, `navigation.rs`) ask it of the file they're about.
- `Status::Absent` becomes `Missing(Missing)`. `Status::describe` names
  the server.
- `AppState` gains `server_installs: RwSignal<HashMap<ServerKind,
  InstallState>>`, app-wide since an install is: `Running`, or `Failed {
  error }` until the next try.
- An external tab's source is a path or a `jdt://` URI.
- Nothing is stored in the database. No schema change.

## perch-lsp

- `ServerKind { RustAnalyzer, Gopls, TypescriptLanguageServer, Jdtls }`,
  with `name()`, `for_path(path) -> Option<ServerKind>`,
  `language_id(path)`, `markers()`, `options(root)`,
  `initialize_timeout()` and `pin()`.
- `find_server(language, root)` becomes `find(kind, root) ->
  Result<ServerSpec, Missing>`. `Missing { kind, needs: Option<Need>,
  install: Option<Install> }`. `Install` is `Command { program, args, env,
  into }` or `Download { url, sha256, into }`. `find` never downloads.
- `ServerSpec` loses `language_id`: `did_open` takes it from the path.
- `Client::start` uses the spec's initialize timeout.
- `handle_notification` handles `window/showMessage` and `language/status`
  as in [Health](#health).
- `Client::class_file_contents(uri) -> Option<String>`.
- `install(Install) -> Result<PathBuf, Error>`: runs or downloads into
  `.partial`, verifies, renames. A checksum mismatch deletes what it
  downloaded and fails with both hashes.
- Every child gets the environment `main` sets up (the stripped rustup pin
  and the extended `PATH`).

## Backend

- `language_server: Arc<Mutex<Option<Arc<Client>>>>` becomes a map from
  `ServerKind` to client, and `resolve` picks by path.
- `start_language_server(ctx, navigation_id)` starts the kinds for the
  open item as in [When a server starts](#when-a-server-starts). A new
  command, `StartLanguageServer { navigation_id, kind }`, comes from the
  UI on a file open.
- `InstallLanguageServer { kind, toolchain: Option<String> }`, with the
  events `LspInstalling { kind }`, `LspInstalled { kind }` and
  `LspInstallFailed { kind, error }`.
- `Event::LspStatus` and `Event::LspWarning` carry the `ServerKind`.
- Startup removes `.partial` directories, superseded versions and orphaned
  jdtls data. The worktree removal removes that worktree's jdtls data.

## Harness

Unit tests beside the code they test, and headless tests against fake
servers in the style of `RECORDING_SERVER`.

- Registry: each extension's kind and language id, `.tsx` as
  `typescriptreact`, `.jsx` as `javascriptreact`; TypeScript and
  JavaScript resolve to the same kind; each kind's markers.
- Finding: the project's, then `PATH`, then Perch's newest; a
  `rust-analyzer` hard link to a fake `rustup` is skipped; each of
  rustup's two errors gives its install command with the toolchain name;
  any other rustup failure falls back to `PATH`; no `go` gives `needs` Go
  and no Install.
- Starting: a pull request changing a `.go` and a `.rs` file starts both;
  an issue with `go.mod` at the root starts gopls; opening a `.ts` file
  starts the TypeScript server, and a `.js` file after it starts nothing
  new.
- Routing: a hover in a `.go` file reaches the Go fake and not the Rust
  one; a name from prose asks both and merges; a fenced `go` block asks
  only gopls; one server failing leaves the other's gotos working.
- Health: `showMessage` Error is `Unhealthy`; Warning is told once; each
  `language/status` type maps as specified.
- Installing: the command and environment for each kind (`GOBIN`,
  `--prefix`, `--ignore-scripts`); success renames `.partial` and starts
  the waiting windows' servers; failure leaves no directory and gives the
  error line; two Installs run one command; a jdtls download whose
  checksum doesn't match is deleted and fails; startup removes
  `.partial`, superseded versions and orphaned jdtls data.
- Update: an install older than the pin offers Update once a session.
- Java: the initialize timeout; a `jdt://` definition asks
  `java/classFileContents` and opens a tab with its text and label.
- Labels: the Go module cache with a `!`-escaped path, GOROOT, a
  downloaded Go toolchain, `node_modules` with and without a scope, pnpm,
  TypeScript's lib files.
- Syntax: each new extension picks its grammar; each fence name
  highlights.

## Build order

1. `ServerKind` and `find` with Rust only. The backend's map of servers,
   statuses keyed by kind, and the messages naming their server. Nothing
   a user would see changes. Commit.
2. Rust's two rules, `Missing`, the toast with Install, and the install
   runner, for rust-analyzer only. Commit. (Worth landing alone: it fixes
   the pinned-toolchain case that prompted this milestone.)
3. Grammars for Go, JavaScript, TypeScript, TSX and Java. Commit.
4. gopls: finding, Perch's install, the start rules, `showMessage`
   health, Go labels. Commit.
5. typescript-language-server: `tsserver.path`, the `node_modules`
   warning, the labels. Commit.
6. jdtls: the JDK, the download and checksum, data directories,
   `language/status`, the initialize timeout, `jdt://` tabs. Commit.
7. Update and the startup cleanup. Commit.
8. README: Requirements says code navigation covers Rust, Go,
   TypeScript, JavaScript and Java and that Perch offers to install a
   missing server. Setup gains a short **Language servers** section: the
   table of servers and what each needs, where Perch installs them, and
   that only Install and Update download anything. Agent safety, or a
   section beside it, gains a line that language servers run project
   code: rust-analyzer's build scripts and proc macros, and Gradle's
   build when jdtls imports. Data gains `servers/`. Commit.

## Acceptance

- Remove 1.98.1's rust-analyzer (`rustup component remove rust-analyzer
  --toolchain 1.98.1`) and open apache/datafusion PR 25788. The toast says
  `rust-analyzer isn't installed for Rust 1.98.1.` with Install, and its
  tooltip is the rustup command. Press it: `Installing rust-analyzer…`,
  then indexing, then hover works. No timeout message appears at any
  point.
- With `~/.perch/servers` gone and no gopls on `PATH`, open a Go pull
  request (cli/cli has plenty). Install gopls. Hover a function: its
  signature. F12 on `http.Get`: a tab labelled `go net/http`. F12 into a
  dependency: a tab labelled with its module and version. `ls
  ~/.perch/servers/gopls` shows the pin and nothing else.
- Take `go` off `PATH` and open the same pull request: `gopls needs Go`,
  no Install.
- Open a TypeScript pull request in a Perch-made worktree: the
  `node_modules` warning shows once; hover and goto within the repository
  work. Open the same repository under your roots, with `node_modules`
  installed: F12 on an import opens the package's `.d.ts`, labelled with
  its name.
- Open a Java pull request on a Maven project (google/guava). Install
  jdtls: the toast shows the download, then indexing for as long as the
  import takes. F12 on `ImmutableList` in a test opens the source; F12 on
  `java.util.List` opens a `JDK` tab with its text. `git status` in the
  worktree shows nothing jdtls wrote.
- Open a pull request that changes both `.rs` and `.ts` files: both
  servers start, and hover in each file answers from its own. Kill the
  TypeScript server: a TypeScript goto says `typescript-language-server
  failed: …`, and Rust navigation still works.
- Quit Perch during the jdtls download. Relaunch: no `.partial` under
  `~/.perch/servers`, and Install works.
- Pin an older gopls into `~/.perch/servers/gopls/`, alone: the first Go
  pull request of the session offers Update, the next doesn't.
- `cargo test` green, `cargo clippy` clean.

## Things the implementer should not do

- Don't download or install anything without an Install or Update click.
  Finding a server stays offline, with `RUSTUP_AUTO_INSTALL=0`.
- Don't install a project's dependencies, and don't run npm's install
  scripts for Perch's own installs either.
- Don't install Go, Node or a JDK. The only toolchain Perch installs is
  the rustup toolchain a checkout pins, and only from Install.
- Don't write outside `~/.perch/servers` except through rustup: no `npm
  -g`, no `GOBIN` the user owns, no `brew`.
- Don't fall back to another release's rust-analyzer when rustup manages
  the checkout.
- Don't put anything a server writes inside the worktree.
- Don't run a command the toast's tooltip doesn't show.
- Don't add a settings screen or per-repository server configuration.
- Don't run `cargo fmt --all`. Format only new files.
