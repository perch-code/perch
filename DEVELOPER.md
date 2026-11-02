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
