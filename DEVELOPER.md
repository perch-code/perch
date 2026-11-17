# Building from source

You need rustup (the repository pins the toolchain), Python 3 and Xcode's
command-line tools.

```bash
./scripts/bundle.sh
```

This builds `Perch.app` under the cargo target directory. Copy it to
`/Applications`, or run Perch directly:

```bash
cargo run --release -p perch-app
```

If you change Perch and run it often, use the `local` profile. It skips
link-time optimization and builds incrementally, for a slightly slower app.

```bash
cargo run --profile local -p perch-app
```

## Engines for hosts

The app carries `perchd` for every host it can reach, inside its own
binary, and starts the one a host needs there. Building the app builds
them: Linux and macOS, on x86_64 and arm64, with the app's profile, into
`target/engines`. So the first build, and any build after a change to the
engine, builds `perchd` up to four times before the app.

- On a Mac, the two macOS ones need nothing more. The two Linux ones need
  zig and cargo-zigbuild: `brew install zig && cargo install cargo-zigbuild`.
- On Linux, the two Linux ones need zig and cargo-zigbuild too; with
  neither, this machine's own is built with `musl-gcc` (`musl-tools`) when
  that is there. The macOS ones need Apple's SDK, with `SDKROOT` naming
  it, and are left out without.
- One that can't be built here is left out with a warning, and the app
  builds `perchd` from this checkout for such a host when it connects.
- `PERCH_ENGINES=none cargo run -p perch-app` builds none: for working on
  the app alone. `cargo clippy` and rust-analyzer build none either;
  `cargo check` does, as cargo gives a build script no way to tell.
- `PERCH_ENGINES=/whole/path` takes the four
  `perchd-<linux|macos>-<x86_64|arm64>.tar.gz` there, as the release
  workflow does.

See [How an app gets its
engines](docs/reference.md#how-an-app-gets-its-engines).
