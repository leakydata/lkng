#!/usr/bin/env bash
#
# Snapshot a contract's current wasm before a change that will move its
# address.
#
# A contract's address is BLAKE3(BLAKE3(wasm) || params). Any change to the
# compiled output — a new struct field, a dependency bump, a different
# compiler — moves it. The old address keeps everyone's data; the new one
# starts empty; nothing errors. Run this *before* the change, while the old
# binary still exists, because afterwards it does not.
#
# Usage: scripts/snapshot-contract.sh <contract-name>
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
NAME="${1:-}"
[ -n "$NAME" ] || { echo "usage: $0 <contract-name>" >&2; exit 1; }

SRC=$(ls "$ROOT/contracts/$NAME/target/wasm32-unknown-unknown/release"/*.wasm 2>/dev/null | head -1)
[ -n "$SRC" ] || {
  echo "FAIL: no built wasm for '$NAME'. Build it first -- snapshotting" >&2
  echo "      a stale or missing binary is worse than not snapshotting." >&2
  exit 1
}

mkdir -p "$ROOT/contracts/legacy"

# Next free version number for this contract.
N=1
while [ -e "$ROOT/contracts/legacy/$NAME-$N.wasm" ]; do N=$((N + 1)); done
DEST="$ROOT/contracts/legacy/$NAME-$N.wasm"

cp "$SRC" "$DEST"
HASH=$(b3sum "$DEST" 2>/dev/null | awk '{print $1}' || sha256sum "$DEST" | awk '{print $1}')

cat >> "$ROOT/contracts/legacy_contracts.toml" <<EOF

[[contract]]
name = "$NAME"
version = $N
retired = "$(date +%Y-%m-%d)"
reason = "FILL THIS IN -- what changed, and what data is at risk"
wasm = "legacy/$NAME-$N.wasm"
hash = "$HASH"
EOF

echo "snapshotted $NAME v$N -> $DEST"
echo
echo "Now do two things, in this order:"
echo "  1. write the 'reason' in contracts/legacy_contracts.toml -- a future"
echo "     reader needs to know what data is in the old address"
echo "  2. commit the wasm. It is intentionally not git-ignored: a migration"
echo "     path that needs a months-old toolchain rebuilt to byte-identical"
echo "     output is not a migration path."
