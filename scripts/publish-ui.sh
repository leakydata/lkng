#!/usr/bin/env bash
#
# Build the web UI and publish it as the LKNG web-container contract.
#
# ## Why this script exists, and why it deletes the output directory
#
# `dx bundle --release` will happily re-emit a *previous* build's wasm while
# reporting success. On 2026-08-01 that cost most of a day: the phone was
# repeatedly declared "stale", its node was force-refetched, the contract was
# republished several times — and every one of those publishes shipped the
# same old bytes, because the build step had never actually run. The network
# was doing exactly what it was told.
#
# The lesson is not "dx has a bug". It is that a build pipeline which can
# silently emit stale output turns every downstream diagnosis into fiction:
# every conclusion drawn about caching, propagation and eviction that day was
# reasoned correctly from a false premise. So this script does two things
# that cost seconds and remove a whole class of wrong answers:
#
#   1. deletes the bundle output directory, so nothing can be inherited;
#   2. stamps a unique marker into the source and **fails the publish** if
#      that marker is not present in the compiled wasm.
#
# (2) is the important half. It makes "did this build actually happen?" a
# checked fact rather than an assumption, and the marker is rendered in the
# UI footer so the same question can be answered *on the device*, by looking,
# without adb.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
OUT="$ROOT/web/target/dx/lkng-web/release/web/public"
MARKER="b$(( $(date +%s) % 100000 ))"

# Stamp the marker.
#
# Done in python, not sed: the line contains `&str`, and `&` in a sed
# replacement means "the entire match", so `sed` spliced the old line back
# into itself and produced code that would not parse. Caught only because the
# release build failed loudly -- had the marker been in a string that still
# compiled, this script would have gone on certifying builds while quietly
# corrupting the file it was checking.
MARKER_LINE='pub const BUILD_MARKER: &str = '
grep -q "^$MARKER_LINE" web/src/main.rs \
  || { echo "FAIL: BUILD_MARKER declaration not found in web/src/main.rs" >&2; exit 1; }
python3 - "$MARKER" <<'PY'
import re, sys
marker = sys.argv[1]
p = 'web/src/main.rs'
s = open(p).read()
s, n = re.subn(r'^pub const BUILD_MARKER:.*$',
               f'pub const BUILD_MARKER: &str = "{marker}";', s, count=1, flags=re.M)
assert n == 1, "marker stamp did not apply"
open(p, 'w').write(s)
PY
grep -q "\"$MARKER\";" web/src/main.rs \
  || { echo "FAIL: marker stamp did not apply" >&2; exit 1; }

# Contracts first, always.
#
# The web bundle embeds the compiled contracts with `include_bytes!`, and a
# contract's key is BLAKE3(BLAKE3(wasm) || params) -- so a stale embedded
# wasm means the app addresses contracts that no longer exist, and every
# read comes back empty. Building in the other order produces a UI that
# looks perfectly healthy and can talk to nobody.
echo "==> building contracts"
for c in presence-cell inbox profile moderation album; do
  [ -d "$ROOT/contracts/$c" ] || continue
  ( cd "$ROOT/contracts/$c" \
    && CARGO_TARGET_DIR="$PWD/target" cargo build --release \
         --target wasm32-unknown-unknown >/dev/null 2>&1 ) \
    || { echo "FAIL: contract $c did not build" >&2; exit 1; }
done

echo "==> building UI (marker $MARKER)"
rm -rf "$ROOT/web/target/dx/lkng-web/release"
( cd web && dx bundle --platform web --release >/dev/null )

WASM=$(ls "$OUT"/assets/*.wasm 2>/dev/null | head -1)
[ -n "$WASM" ] || { echo "FAIL: no wasm produced" >&2; exit 1; }

# The gate. A build that does not contain the marker we just stamped is a
# stale build, and publishing it would put us back where the day started.
if ! grep -qa "$MARKER" "$WASM"; then
  echo "FAIL: built wasm does not contain marker $MARKER -- dx served a cached artifact." >&2
  echo "      Refusing to publish. Re-run; if it persists, 'dx clean' in web/." >&2
  exit 1
fi
echo "    verified: $(basename "$WASM") contains $MARKER"

# Publish to every node given on the command line, defaulting to the local
# one. A phone reachable over `adb forward` is just another --node-url.
NODES=("$@")
[ ${#NODES[@]} -gt 0 ] || NODES=("")

for node in "${NODES[@]}"; do
  label="${node:-local}"
  echo "==> publishing to $label"
  if [ -z "$node" ]; then
    fdev website update --key lkng "$OUT" --timeout 400
  else
    fdev --node-url "$node" website update --key lkng "$OUT" --timeout 400
  fi
done

echo
echo "published build $MARKER"
echo "The footer of the running app shows this string. If the device shows"
echo "anything else, the device is genuinely stale -- and only then."
