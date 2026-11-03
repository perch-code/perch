# Releasing

Bump `version` under `[workspace.package]` in `Cargo.toml`, run `cargo check`
to update `Cargo.lock`, commit both, then push a matching tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The release workflow builds `Perch.app` for Apple Silicon and Intel and
publishes both tarballs to a GitHub release. A tag with a hyphen, like
`v0.2.0-beta.1`, becomes a prerelease, which neither the updater nor
`install.sh` picks up.
