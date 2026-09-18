#!/usr/bin/env bash
# Rebuild the macOS icon from the vector artwork in assets/perch.svg.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir "$WORK/Perch.iconset"
# Install librsvg (for example, `brew install librsvg`) to render the SVG.
command -v rsvg-convert >/dev/null || { echo "rsvg-convert is required (brew install librsvg)" >&2; exit 1; }
rsvg-convert -w 1024 -h 1024 "$ROOT/assets/perch.svg" -o "$WORK/source.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$WORK/source.png" --out "$WORK/Perch.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$WORK/source.png" --out "$WORK/Perch.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/Perch.iconset" -o "$ROOT/assets/Perch.icns"
echo "Built assets/Perch.icns"
