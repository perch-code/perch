# Perch: Milestone 26, An engine on another machine

## Goal

Perch is one process. The window, the GitHub client, the cache, the
checkouts, the language servers and the agents all live on the Mac in
front of the reader, and closing the lid stops all of it. The reader
may have an always-on Linux machine where builds are fast,
dependencies stay installed and agents can keep working overnight.

After this milestone:

- Everything below the UI is `perch-engine`, a crate with no floem, GTK
  or platform UI in it. The Mac app runs it in process, as today, with
  no change in behaviour.
- `perchd` runs the same engine headless, as a long-lived service on
  the box. It owns the GitHub client, `perch.db`, the store, the
  checkouts, the language servers and the runs, and keeps all of it
  going with no window connected.
- The Mac app can connect to a `perchd` instead of starting its own
  engine. The connection is `ssh <host> perchd bridge`: SSH does the
  authentication, and nothing listens on a network port.
- A window that loses its engine says so, reconnects, and catches up.
  Runs on the box carry on in the meantime.

This reverses three earlier non-goals, on purpose:

- Milestone 7: "there is one environment and it is this machine". There
  are now two kinds of engine, in process and on a host. Still one at a
  time per app.
- Milestone 12: "background daemon, remote execution". `perchd` is a
  background service, and its runs execute on another machine.
- Milestone 12: "Reattachment across app restarts is out of scope". A
  client that reconnects to `perchd` sees the runs that kept going. A run
  still never outlives its engine: `perchd` stopping interrupts its runs,
  as quitting the app does today.

## Non-goals

- A second implementation of the backend. The engine is today's
  backend, moved. The protocol is today's `Command` and `Event`. The
  unmerged `perch-server` branch built a separate engine with its own
  RPC set; that is the thing this milestone avoids.
- One sidebar across several engines. The app talks to one engine at a
  time: This Mac, or a host.
- A network listener, TLS, or a phone client. The transport is a byte
  stream; a WebSocket can carry it later.
- Sandboxes, containers and VMs (the M25 branch's providers), and
  per-repository setup recipes. Both slot in under the engine later.
- Moving code between engines. An item lives where its engine is.
- Windows hosts.

## Architecture

```
Mac                                     host (Linux, always on)
perch-app (floem)                       perchd serve
  windows, editor, rendering    ssh     ├─ unix socket $PERCH_HOME/run/perchd.sock
  image and SVG decoding   ───────────▶ │   (perchd bridge copies stdio to it)
  client settings (ui, display,  NDJSON └─ perch-engine
  keybindings, remote)     ◀─────────── GitHub, perch.db, store, worktrees,
                                         runs, LSP, snapshots, approvals
```

### Crates

- `perch-engine`: `perch-app/src/backend/`, and the logic it uses that
  lives elsewhere today (`ask`, `approvals`, `context`, `items`, the
  feedback and update logic, `repos`, `inbox`'s helpers, `log_file`),
  and the data types it shares with the UI, cut out of
  `review/model.rs`, `review/suggestion.rs`, `review/code_refs.rs` and
  `state.rs`. It depends on the other workspace crates and on no UI
  toolkit. `cargo tree -p perch-engine` has no floem, winit, gtk or muda.
- `perch-engine::wire`: the handshake, the framing, and the transport
  that carries `Command` and `Event` over a byte stream. Kept in the
  engine crate so the types and their wire form cannot drift apart.
- `perchd`: a binary crate with `serve`, `bridge` and `status`.
- `perch-app`: the UI. Where it said `crate::backend::…` it still does,
  through a re-export.

### The boundary

The UI talks to the engine through `Backend`, the handle it has today,
and hears it on the `(WindowId, Event)` channel it has today. A
`Backend` is built either by `perch_engine::spawn()` (in process) or by
`perch_engine::wire::connect(stream)` (remote). Nothing else in the UI
knows which.

Nothing the UI and the engine exchange may be anything but data:

- `Command` and `Event` and every type they carry derive `Serialize` and
  `Deserialize`.
- No shared memory crosses the boundary. The fences the handle shares
  with the worker today (`NavigationRequests`, `ScopeRequests`,
  `MergeAttempts`, `PullRequestReviewAttempts`) become ids carried in
  commands. The engine keeps its own copy of each and updates it when the
  command arrives; the UI drops a result whose id is not the latest it
  issued. Over a wire a stale result can now reach the UI, so every
  handler that takes a navigation, scope or attempt id checks it.
- Pictures travel as bytes. The engine never decodes an image into a
  brush or parses an SVG into a render tree; the UI does, when it
  draws.
- The UI never reads the engine's files. `windows::load_frame` keeps
  reading the window frame, which is the client's own.

### The wire

Newline-delimited JSON, one message per line, on any duplex byte stream.

- The client sends `Hello { protocol, app_version }`. The engine answers
  `Welcome { protocol, engine_version, host, home, data_root }` or
  `Refused { reason }`. A different `protocol` number is refused before
  any command runs, with both versions named.
- Then the client sends `{ window, command }` and the engine sends
  `{ window, event }`.
- Window ids are per connection. The engine maps a connection's window
  to an engine window of its own, and maps events back. An event for
  `NO_WINDOW` goes to every connection; an event for a window goes only
  to the connection that owns it.
- `Command::Resync` asks the engine to send, to that connection, what it
  sends every window at startup: repositories, items, notifications,
  settings, runs, usage, pending approvals. A client sends it after
  `Welcome`, and again after every reconnect, then reopens each window
  and the item it had open. There is no replay buffer: a reconnect is a
  resync.
- Logs never go on the stream. `perchd` logs to stderr and to its log
  file.

`PERCH_WIRE_LOOPBACK=1` makes the in-process app send every command and
event through JSON and back, so a regression in the wire form shows on
the Mac without a host.

### perchd

- `perchd serve` runs the engine and listens on
  `$PERCH_HOME/run/perchd.sock` (directory 0700, socket 0600). It holds
  an exclusive lock on `$PERCH_HOME/engine.lock` for its lifetime. The
  in-process app takes the same lock, so two engines never share one
  `perch.db` (the second would mark the first one's runs Interrupted).
- The engine keeps its own command sender, so the last client leaving
  closes nothing. Polling, runs, sweeps and checks carry on with zero
  clients.
- SIGTERM and SIGINT shut down as quitting the app does: runs are
  stopped and recorded Interrupted, children are reaped.
- `perchd bridge` connects its stdin and stdout to the socket. If no
  engine is listening it starts `perchd serve` detached (its own session,
  output to the log) and waits for the socket. This is the only way an
  engine starts on a host: no service manager, so nothing runs there
  until the first connection after a boot.
- `perchd status` says whether an engine is running, its version and
  how many clients and runs it has.
- `perchd` does not link GTK. Its release artefact is a Linux tarball
  beside the app's.

### The client

- `[remote] host` in the Mac's config.toml, or Settings > General >
  Engine, names an SSH host. Empty means This Mac. `[remote] command`
  defaults to `perchd bridge` and is run through the remote login shell.
- The app runs `ssh -T -o ServerAliveInterval=15 -o
  ServerAliveCountMax=3 <host> <command>`. SSH's own config, keys and
  agent apply.
- A lost connection shows in every window as a lasting banner, not a
  toast: "Disconnected from <host>. Reconnecting…", with a Reconnect
  button. The app retries with backoff (1s doubling to 30s), resyncs and
  reopens what each window had open. Commands sent while disconnected
  are refused at the handle with a message, never queued.
- A protocol mismatch says which side to update, and does not retry.
- What opens something on the Mac follows the engine:
  - Open in Terminal runs `ssh -t <host> 'cd <path> && exec $SHELL -l'`
    in Terminal.
  - Reveal in Finder becomes Open in Editor: `zed ssh://<host>/<path>`
    when Zed is installed, else VS Code's `code --remote
    ssh-remote+<host> <path>`.
  - Copy Path copies the host's path.
  - Paths shown are the host's, and `~` means the host's home
    (`Welcome.home`).

### Settings

Settings split by whose they are:

- The client's, in the Mac's config.toml, read and written locally
  whatever the engine: `[ui]`, `[display]`, `[keybindings]`, `[remote]`.
- The engine's, in the engine's config.toml, read and written by the
  engine: the token, `[[repo]]`, `[agent]`, `[work]`, `[lsp]`,
  `poll_interval_secs`, `github_host`.

In process both are the one file, as today.

### Approvals

A pending approval is engine state, not a one-shot event. It is in the
resync, so a client that connects later shows the card. The wait before
a request is denied is `[work] approval_timeout_mins`: 15 by default in
the app, 480 under `perchd`, because nobody may be connected.

### Linux

What the host needs that the Mac didn't:

- ty is downloaded for the host's triple, not `apple-darwin`.
- Language-server memory comes from `ps -eo pid=,ppid=,rss=` on Linux.
- Claude Code's sandbox needs `bwrap` and `socat` on Linux. A Work or
  Ask run on a host without them is refused with that sentence, not
  with Claude Code's exit.
- Claude Code on Linux skips glob `denyWrite` patterns, so a Work run
  loses `*/**/.git`. After a Work run on Linux, a `.git` that appeared
  below the worktree's top fails the run's record with the path named.

### Beside T3 Code

T3 makes its worktrees with `git worktree add` from the project root,
so a `[[repo]] root` that names the same clone lists them as worktree
items already, and Perch never resets or removes a worktree it does not
own. T3 renames a thread's branch after creating it. Today the scan
archives the old row and makes a new one; it now renames the row's branch
instead (see As built for how a rename is told).

## Build order

All four steps are done.

1. **Engine crate.** (Done.) Move the backend and its support into
   `perch-engine`; the app re-exports it. Pictures become bytes. No
   behaviour change; every existing test passes where it moved to.
2. **Wire-ready boundary.** (Done.) serde on both enums and their payloads,
   fences as ids, per-connection window mapping, `Resync`, the framing
   and handshake, the loopback mode and its test.
3. **perchd and the client.** (Done.) `serve`, `bridge`, `status`, the lock, the
   socket, the unit file; the app's remote mode, banner and reconnect,
   the remote openers, the settings split; the Linux fixes.
4. **Durable approvals and T3.** (Done.) Approvals in the resync and their
   timeout; the scan's rename by path; docs.

## As built

Where the implementation went its own way, and why:

- The wire carries more than the spec named. `Welcome` also has the
  connection's number (attempt ids carry it), and how many other clients
  and runs the engine has. An empty line is a keepalive: the client sends
  one every 15 s it has nothing else to send, and the engine hangs up on a
  client it has heard nothing from in 60 s, closing its windows, so a Mac
  asleep on a half-open connection does not hold windows and language
  servers for the hours TCP takes. The engine also hangs up on a client
  more than 256 MB behind in what it is sent. The protocol is 4.
- Fences are taken as a command is sent, at the handle and the hub, not
  when the engine's loop reaches it, so a window's commands are fenced in
  the order it sent them. Scope picks are numbered at the handle.
- Lines a login shell prints before the bridge starts (a greeting, a
  terminal title) are passed over, up to 64, before the engine's answer.
- ssh runs with `BatchMode=yes`, `ConnectTimeout=10` and every forwarding
  off (`ForwardAgent=no`, `ForwardX11=no`, `ClearAllForwardings=yes`)
  whatever the ssh config says, and the command goes through
  `exec "$SHELL" -lc`. `perchd` also drops `SSH_AUTH_SOCK`,
  `SSH_CONNECTION`, `SSH_CLIENT` and `SSH_TTY` from its environment when
  started in an ssh session, since it outlives the session and its agents
  would inherit them; outside one, the agent socket is the host's own.
- Reconnects give up a try not answered in 30 s, and a connection lost
  within a minute of being made does not reset the backoff. What a window
  asked for while the very first try was under way is sent when it
  connects, or said not done when it fails; after that, commands while
  disconnected are refused as specified.
- `perchd serve` stops on SIGHUP too, a second signal ends it at once,
  and it exits 75 when another engine has the home. `perchd status` exits
  3 when nothing answers; `perchd --version` names the protocol.
- Open in Editor prefers Zed when its command or app is installed, and
  says so when neither Zed nor VS Code can be run.
- Add repository… does not reach the host: with a host set it says to add
  the `[[repo]]` to the host's config.toml by hand.
- The nested `.git` check on Linux also fails a run whose existing nested
  `.git` had its config or hooks changed, or whose `.git` file points
  elsewhere, not only one that appeared.
- The window frame moved out of the database into `window-frame` in the
  Mac's own Perch home, read and written there whatever the engine; the
  old meta key is still read when there is no file. Remote, the frame had
  been written to the host's database and never read back.
- Pending approvals go in the resync as `Event::Approvals`, which replaces
  a window's cards, so one settled while a client was away does not linger.
  The timeout's default comes from the engine's `Lifetime`, and the denial
  names the actual wait. `[work] approval_timeout_mins` has no control in
  Settings > Work, which lists no other numbers.
- The scan does not match by path: a branch switched and the old one
  deleted before the next scan looks the same as a rename from the
  worktree list. It renames a row only when the new branch's reflog says
  `git branch -m` renamed it from the row's branch, which is gone. A hidden
  row is renamed and stays hidden; a Perch-made row has only its branch
  renamed; a row with a pull request is not renamed. Otherwise the old row
  is put away as before, as is one renamed onto a branch another row holds.
- The hello has the same minute as the keepalives. The loopback wire has
  no idle limit, and the app opts out of App Nap while on a host.

## Provisioning the host

Added after the first build. A bridge is the only way an engine starts
on a host: there is no systemd unit. The app installs the `perchd` that
matches it over the same ssh it connects with, so a host needs nothing
from Perch beforehand, and updating the app updates the host on its next
connect. VS Code's Remote SSH, Zed and T3 Code work the same way.

### Build order

1. A static `perchd`: the release builds it for
   `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` (rustls
   and bundled SQLite, no OpenSSL), checks it is static, and publishes a
   `SHA256SUMS` beside the assets. `perchd-linux-<arch>.tar.gz` keeps its
   name. (Done.)
2. Probe and upload (below). (Done.)
3. Versioned installs and their cleanup (below). (Done.)
4. Replacing an older engine (below). (Done.)
5. Host checks (below). (Done.)

### Modes

- `[remote] command` unset or empty: provisioned. Set: run as given, as
  before, with no install, replace or cleanup. `command` becomes an
  `Option`; a config that spells out `command = "perchd bridge"` keeps
  the old behaviour.
- `[remote] perchd = "<path on the Mac>"`: a Linux `perchd` binary or
  release tarball to upload instead of downloading, for dev builds and
  hosts with no release. The default downloads the app's own version.
- `PERCH_SSH`: the ssh program, for tests (a wrapper adding `-F`).

### Layout on the host

`${PERCH_HOME:-$HOME/.perch}/bin/perchd-<build>`, directory 0700, file
0700. `<build>` is the version (`0.9.0`) for a release download, and
`<version>-<first 12 hex of its SHA-256>` for a `[remote] perchd` file,
so a rebuilt dev binary is uploaded again. The app always knows the
`<build>` it wants before it dials.

### Probe and upload

- In provisioned mode the login shell runs a POSIX `sh` script, passed
  the build as `$1`: run `$dir/perchd-$1 bridge` if it is executable,
  else write `perch-install: <uname -s> <uname -m>` to stderr and exit
  97. It must work with bash, zsh and fish as the login shell, so the
  outer quoting holds no single quotes and the logic is all in `sh`.
- On that line the link installs and redials at once, outside the
  backoff. `Linux x86_64` maps to `linux-x86_64`, `Linux aarch64|arm64`
  to `linux-arm64`; anything else is an error naming what was found and
  saying to install `perchd` by hand and set `[remote] command`.
- The tarball is cached on the Mac in
  `~/.perch/perchd/<version>/perchd-linux-<arch>.tar.gz`, downloaded with
  no token from
  `https://github.com/perch-code/perch/releases/download/v<version>/`
  (the repository is public) and checked against that release's
  `SHA256SUMS`. A release with no such asset is an error that says to
  set `[remote] perchd`.
- A second ssh uploads the binary on stdin to a script that writes it to
  a temporary file in the directory (umask 077), runs it with
  `--version`, and renames it into place only when that names the
  expected version and protocol. The temporary file is removed on any
  failure.
- The link strip says "Installing perchd 0.9.0 on <host>…" meanwhile.
- `perch --connect <host>` does all this headless, prints the welcome
  and the host checks, and exits: for testing and for the reader.

### Cleanup

A provisioned bridge (its own exe is `$dir/perchd-*`) removes the other
`perchd-*` in its directory but the newest one, and temporary files over
an hour old. A running engine's binary can be removed on Linux.

### Replacing an older engine

- The hello and the answer are frozen from here: later protocols only
  add optional fields. `Hello` gains `intent`, `Connect` (the default)
  or `Stop { if_idle }`. The refusal gains the engine's version,
  protocol, build, runs and clients, and the welcome its build (the
  exe's `<build>`, or its version when not provisioned). `PROTOCOL`
  goes to 5.
- An engine under `perchd serve` answers `Stop` whatever the protocol:
  `Busy` with its runs and clients when `if_idle` and it has either,
  else `Stopping`, and then stops as SIGTERM does, runs recorded
  Interrupted. The app's own engine never stops for the wire.
- `perchd bridge --replace` and `--replace-if-idle` send that stop to an
  engine of another build, wait up to a minute for the home to be let
  go, then start their own and carry on as a bridge. A busy engine makes
  `--replace-if-idle` write `perch-busy: <version> <runs> <clients>` to
  stderr and exit 98. An engine of the same build is just joined.
- The app, provisioned, compares the build it wants with the engine's.
  Older, with no runs and no other client: it redials with
  `--replace-if-idle`. Older and busy: if the protocol matches it stays
  connected and the strip says the engine is older, with how many runs a
  restart would stop and a Restart button (`--replace`); if it does not
  match, the same strip, without the connection. An engine newer than
  the app is never stopped: the strip says to update Perch on the Mac.
- An older `perchd` refuses a `perch.db` a newer one migrated (the
  cache's newer-schema error). The bridge says why an engine it started
  did not answer, from what `serve` left in `$PERCH_HOME/run`, rather
  than only pointing at the log.

### Host checks

The engine under `perchd serve` works out what the host lacks, at start
and when its config changes, and sends it in the resync: a GitHub token,
`git`, a `[[repo]]` whose root is on the host, each agent CLI, on Linux
bubblewrap and socat when Claude Code is there, and logind's
`KillUserProcesses=yes`. Each says what to run to fix it. Settings >
General > Engine lists them; the strip says "<host> needs setup" with a
button to Settings when anything required is missing.

### As built

- The static check asks `readelf` for an interpreter or a `NEEDED`
  library rather than reading `file` or `ldd`, which word a static binary,
  and a static PIE, differently from one version to the next. CI's Linux job also builds `perchd` for
  `x86_64-unknown-linux-musl` and runs the same check, so a dependency
  that needs OpenSSL or glibc fails on the pull request, not at release.
- What ssh runs is `exec "$SHELL" -lc 'exec /bin/sh -c "<script>"
  perch-connect <build>'`. The script has no single quote, backslash,
  backquote or `!`, and each `$` and `"` in it is behind a backslash,
  which bash, zsh and fish all read as the character itself inside double
  quotes. A test runs the exact line through each shell it finds, against
  a temporary `HOME` with a stand-in `perchd-<build>`, and the upload
  script the same way.
- An install that goes through, after which the redial still says
  `perch-install`, stops the tries (no retry until Reconnect) and says the
  host still finds none, most likely a login shell with another
  `PERCH_HOME`. A host Perch has no build for, and a release with no
  asset, stop them too. A download or upload that fails waits for the next
  try as any failure does, and that try installs again.
- The `perch-install` line is kept as ssh's last line even when ssh says
  something after it, a shared connection closing say.
- The upload checks for the app's version and protocol whatever the
  source, so a `[remote] perchd` from another version is refused there.
  Its processor is checked on the Mac first, from the ELF header.
- The tarball is read in Rust (flate2 and a small tar reader) rather than
  by `tar`, so a test needs no tar, and macOS's `._perchd` beside it in a
  tarball made on a Mac is passed over. `SHA256SUMS` is kept beside the
  cached tarball, and the cache is used only while the two still agree.
- Its `<build>` hashes the binary after it is taken out of a tarball, so a
  tarball and its binary are one build. The file is read again on each
  try, so a rebuilt one is installed on the next connect.
- `PERCH_SSH` is honoured by Open in Terminal too, not only the link and
  the upload.
- The link's new state is `EngineLink::Installing { host, build }`; after
  an install the strip says Connecting again until the engine answers.
- Cleanup runs in a thread as the bridge starts, keeps the newest other
  `perchd-*` by modification time, and touches only plain files.
- `perch --connect` prints the welcome only; the host checks came with
  step 5.
- The stop hello also carries `unless_build`, the bridge's own build, and
  an engine of that build answers `Staying`. The bridge's check and its
  stop are one exchange, so two Macs of the same new build replacing the
  same old engine at once cannot have the second stop the engine the first
  just started. `Busy`, `Stopping` and `Staying` each carry the engine's
  `About` (version, protocol, build, runs, clients), which a refusal
  carries beside its reason.
- The engine answers the stop in `wire::serve`, which only tells the
  engine (`Engine::ask_to_stop`); `perchd serve` waits for that beside its
  signals and stops down the same path, so there is one way to stop.
- An engine from before protocol 5 says nothing of itself in a refusal
  and refuses a stop as it refuses any other protocol. The app does not
  dial again to try: the strip says to stop that `perchd serve` by hand.
- `perchd serve` refuses a `perch.db` from a newer Perch at start, rather
  than running without its database as the app's engine does; a host's
  engine has nobody to tell, and its runs would go unrecorded. Its reason,
  and any other a serve fails with, goes in `$PERCH_HOME/run/serve-failed`.
  A bridge whose serve failed with a reason there (any exit but 75, the
  home held) says it at once rather than waiting out its 10 s.
- The strip's states are `EngineLink::Restarting { host, build }`, and
  `other: Option<OtherEngine>` on `Up` and `Down`. With runs at 0 the
  strip names the other clients a restart would disconnect instead.
- One automatic replace a connect: the count starts again on a connection
  to the build wanted, or when the reader presses Reconnect or Restart. A
  try that replaces waits a minute longer for the engine's answer, the
  minute the bridge gives the old engine to stop.
- `perch --connect` replaces an idle older engine too, and prints what it
  left running. `perchd status` prints the engine's build. The protocol
  is 5.
- The host checks are `Event::HostChecks(Vec<HostCheck>)`, each with
  `what`, `ok`, `required`, `detail` and an optional `fix`, and the
  protocol is 6 for it. The engine keeps the last look for a resync and
  sends a look only when it differs from the last, so a config taken up
  with nothing changed for the host says nothing. A look begun later wins
  over a slow one begun earlier. It also looks again when a poll takes up
  a new token, since `gh auth login` changes no config.
- logind is asked with `loginctl show-session -p KillUserProcesses -p
  KillOnlyUsers -p KillExcludeUsers`, which with no session names the
  manager's settings; `show-user` has no `KillUserProcesses`. The check
  weighs the three as logind does, so a user in `KillExcludeUsers` passes,
  and the fix adds the user there in a drop-in rather than turning the
  setting off for everyone.
- The agents come from discovery's answer: an agent is found when
  discovery put a path in place of its name. Nothing looks for them a
  second time. One agent is enough, so no agent check is required unless
  none is found, and then Claude Code's is.
- The fixes that install use the first of apt-get, dnf, yum, pacman,
  zypper and apk found on the host. A missing `[[repo]]` root is cloned
  from github.com only when the config names no GitHub Enterprise host,
  since which host a repository is on is not known from its name.
- `perch --connect` sends a resync after the welcome and waits up to 30 s
  for the checks; an engine that has just started looks after discovery.
- The strip shows "needs setup" only while connected to the build the app
  wants; an engine of another build has its own strip, which matters
  more.

## Acceptance

- `cargo tree -p perch-engine -e normal` names no floem, winit, gtk,
  muda or objc2 crate.
- With no `[remote] host`, the app behaves as before, and with
  `PERCH_WIRE_LOOPBACK=1` it behaves the same.
- A test starts `perchd serve` on a temporary `PERCH_HOME`, connects
  two clients through `perchd bridge`, opens an item from one, drops
  it, and the other still gets events; a third client's resync lists the
  items.
- A second `perchd serve`, or the app in process, on the same
  `PERCH_HOME` refuses to start and says why.
- On the host: open a pull request from the Mac, start a Work run, quit
  the Mac app, reopen it, and the run's progress and result are there.

## Things the implementer should not do

- Write a second engine, or a second set of messages beside `Command`
  and `Event`.
- Queue commands across a disconnect.
- Put the GitHub token on the wire. The host's engine resolves its own,
  as the app does today.
- Forward the SSH agent, or listen on a TCP port.
