#!/usr/bin/env bash
# Build a Finder-launchable app with the Perch icon.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build --release -p perch-app
TARGET="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
APP="$TARGET/Perch.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$TARGET/release/perch" "$APP/Contents/MacOS/perch"
cp assets/Info.plist "$APP/Contents/Info.plist"
cp assets/Perch.icns "$APP/Contents/Resources/Perch.icns"
codesign --force --sign - "$APP"
echo "Built $APP"
