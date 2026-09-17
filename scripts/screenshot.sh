#!/usr/bin/env bash
# Capture the running Perch window to a PNG.
#
# Requires the screen to be unlocked: macOS will not composite an app window
# while the login window is up, and window capture fails with "could not
# create image from window".
#
# Usage: scripts/screenshot.sh [output.png] [--wait SECONDS]
set -euo pipefail

OUT="${1:-perch-window.png}"
WAIT=0
if [[ "${2:-}" == "--wait" ]]; then
  WAIT="${3:-600}"
fi

screen_locked() {
  ioreg -n Root -d1 -a 2>/dev/null | grep -q "CGSSessionScreenIsLocked"
}

window_id() {
  /usr/bin/swift - <<'SWIFT' 2>/dev/null
import CoreGraphics
import Foundation
let opts = CGWindowListOption(arrayLiteral: .optionAll)
guard let infos = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] else { exit(1) }
for w in infos {
    let owner = (w[kCGWindowOwnerName as String] as? String) ?? ""
    let layer = (w[kCGWindowLayer as String] as? Int) ?? 0
    let bounds = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
    let width = (bounds["Width"] as? Double) ?? 0
    // Skip the menu-bar item; the app window is the large one at layer 0.
    if owner == "perch" && layer == 0 && width > 400 {
        print((w[kCGWindowNumber as String] as? Int) ?? 0)
        exit(0)
    }
}
exit(1)
SWIFT
}

deadline=$(( $(date +%s) + WAIT ))
while screen_locked; do
  if [[ "$(date +%s)" -ge "$deadline" ]]; then
    echo "screen is still locked; unlock the Mac and run this again" >&2
    exit 2
  fi
  sleep 5
done

WID="$(window_id || true)"
if [[ -z "$WID" ]]; then
  echo "no Perch window found; is the app running?" >&2
  exit 3
fi

screencapture -x -o -l "$WID" "$OUT"
echo "wrote $OUT"
