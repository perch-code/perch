# Releasing

Bump `version` under `[workspace.package]` in `Cargo.toml`, run `cargo check`
to update `Cargo.lock`, commit both, then push a matching tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The release workflow builds `perchd` for Linux and macOS, on x86_64 and
arm64, then `Perch.app` for Apple Silicon and Intel and Perch for Linux,
each with all four engines built into it, and publishes the
apps' tarballs and the engines' to a GitHub release. A tag with a hyphen, like
`v0.2.0-beta.1`, becomes a prerelease, which neither the updater nor
`install.sh` picks up.

To rehearse a release without publishing one, after a change to the
workflow or to how the app takes its engines, run it by hand:

```bash
gh workflow run release.yml
```

It builds the engines and the apps as a tag would and stops there.

## macOS Signing and Notarization

When the repository has Apple signing secrets configured under **Settings › Secrets and variables › Actions**, the release workflow signs `perchd` and `Perch.app` with Developer ID and the hardened runtime (so macOS TCC permissions persist across updates) instead of signing ad hoc:

- `APPLE_CERTIFICATE_P12` — base64-encoded `.p12` export of the `Developer ID Application` certificate and private key (`base64 -i cert.p12`)
- `APPLE_CERTIFICATE_PASSWORD` — export password for that `.p12`
- `APPLE_SIGNING_IDENTITY` *(optional)* — explicit identity name or SHA-1 fingerprint (auto-detected from the imported `.p12` when omitted)

To also notarize and staple `Perch.app` for Gatekeeper on browser downloads, add an App Store Connect API key:

- `APPLE_API_KEY_P8` — contents of `AuthKey_<KEY_ID>.p8` (raw PEM or base64-encoded)
- `APPLE_API_KEY_ID` — Key ID
- `APPLE_API_ISSUER_ID` — Issuer ID
