#!/usr/bin/env bash
# Build a Finder-launchable app with the Perch icon, for this Mac's
# processor or the target triple given (x86_64-apple-darwin).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
TRIPLE="${1:-}"
cargo build --release --locked -p perch-app ${TRIPLE:+--target "$TRIPLE"}
METADATA="$(cargo metadata --no-deps --format-version 1)"
TARGET="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' <<<"$METADATA")"
# The app's version is the workspace's, so a release only bumps Cargo.toml.
VERSION="$(python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "perch-app"))' <<<"$METADATA")"
BIN="$TARGET/${TRIPLE:+$TRIPLE/}release/perch"
APP="$TARGET/Perch.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/perch"
cp assets/Info.plist "$APP/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
cp assets/Perch.icns "$APP/Contents/Resources/Perch.icns"
codesign --force --sign - "$APP"
echo "Built $APP ($VERSION, $(lipo -archs "$APP/Contents/MacOS/perch"))"
