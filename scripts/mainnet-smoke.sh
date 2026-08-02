#!/usr/bin/env bash
#
# Run every mainnet proof, in order, against the live network.
#
# ## Why this exists
#
# Each example here was written because the path it covers had been built,
# reasoned about carefully, and was broken anyway — a delta encoded in a
# shape the contract does not decode, an inbox addressed by a key the sender
# cannot know, a doc comment the network refuted in one round trip. None was
# visible by reading the code.
#
# So these are not demos. They are the only evidence that the write paths
# work, and they need to be runnable as one command before a release rather
# than remembered individually.
#
# ## What it does not tell you
#
# It exercises the *libraries and contracts* against mainnet. It does not
# exercise the app: the UI, the WebView, the Android node lifecycle and the
# battery cost are all outside this, and a green run here says nothing about
# any of them. Two people on two phones remains the test that matters.
#
# Usage: scripts/mainnet-smoke.sh
#        Requires a running local node (freenet network).
set -uo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
C="$ROOT/contracts"
W() { echo "$C/$1/target/wasm32-unknown-unknown/release/$2"; }

# Contracts must be current: the examples embed nothing, but they load these
# files, and a stale one addresses a contract nobody hosts.
echo "==> building contracts"
for c in presence-cell inbox profile moderation album; do
  ( cd "$C/$c" && CARGO_TARGET_DIR="$PWD/target" cargo build --release \
      --target wasm32-unknown-unknown >/dev/null 2>&1 ) \
    || { echo "FAIL: contract $c did not build" >&2; exit 1; }
done

# Cell parameters for the current epoch, so presence lands somewhere live.
PARAMS=$(mktemp /tmp/lkng-cell-params.XXXXXX)
python3 - "$PARAMS" <<'PY'
import sys, time
def u(n, major=0):
    b = major << 5
    if n < 24: return bytes([b | n])
    if n < 256: return bytes([b | 24, n])
    if n < 65536: return bytes([b | 25]) + n.to_bytes(2, 'big')
    if n < 2**32: return bytes([b | 26]) + n.to_bytes(4, 'big')
    return bytes([b | 27]) + n.to_bytes(8, 'big')
def t(s):
    e = s.encode(); return u(len(e), 3) + e
epoch = int(time.time()) // (6 * 3600)
out = u(3, 5) + t('schema_v') + u(1) + t('cell_id') + t('9q8yy') + t('epoch') + u(epoch)
open(sys.argv[1], 'wb').write(out)
print(f"    cell 9q8yy, epoch {epoch}")
PY

PASS=0
FAIL=0
TIMEOUT=0
FAILED=()
TIMEDOUT=()

run() {
  local name="$1" out rc; shift
  echo
  echo "==> $name"
  # Three attempts. Mainnet PUTs time out under peer churn and under load on
  # a node that has been up for hours; observed failing twice in a row and
  # then passing alone, so two was not enough.
  for attempt in 1 2 3; do
    out=$(timeout 560 cargo run -q --example "$name" -p lkng-transport-freenet -- "$@" 2>&1)
    rc=$?
    echo "$out" | sed 's/^/    /'
    if [ "$rc" = 0 ]; then
      PASS=$((PASS + 1))
      return 0
    fi
    [ "$attempt" -lt 3 ] && echo "    (retry $attempt)"
  done

  # Classify. A timeout is the network refusing to carry a write; an
  # assertion or verification failure is the code being wrong. Reporting
  # both with the same alarming sentence trains people to ignore it, which
  # costs exactly when it is the second kind.
  if echo "$out" | grep -qiE 'timed out|Timeout|session reader has stopped'; then
    TIMEOUT=$((TIMEOUT + 1))
    TIMEDOUT+=("$name")
  else
    FAIL=$((FAIL + 1))
    FAILED+=("$name")
  fi
}

run two_apps         "$(W presence-cell presence_cell.wasm)" "$PARAMS"
run tile_to_message  "$(W presence-cell presence_cell.wasm)" "$PARAMS" "$(W inbox inbox_contract.wasm)"
run report_flow      "$(W moderation moderation_contract.wasm)"
run album_share      "$(W album album_contract.wasm)" "$(W inbox inbox_contract.wasm)"
run migrate_forward  "$(W inbox inbox_contract.wasm)"

rm -f "$PARAMS"

echo
echo "-------------------------------------------"
echo "$PASS passed, $FAIL failed, $TIMEOUT timed out"

if [ "$TIMEOUT" -gt 0 ]; then
  printf 'timed out: %s\n' "${TIMEDOUT[*]}"
  echo
  echo "A timeout is the network declining to carry a write, not evidence the"
  echo "path is broken. It happens under peer churn and on a node that has been"
  echo "up for hours. Re-run the named example alone before concluding anything;"
  echo "if it passes there, the path is fine and the node is tired."
fi

if [ "$FAIL" -gt 0 ]; then
  printf 'failed: %s\n' "${FAILED[*]}"
  echo
  echo "This is an assertion or verification failure, which IS a broken write"
  echo "path — every example here was added after the path it covers shipped"
  echo "broken, and none of them fail this way for environmental reasons."
  exit 1
fi
[ "$TIMEOUT" -gt 0 ] && exit 2
echo
echo "All mainnet write paths verified. This says nothing about the app on a"
echo "phone: UI, WebView, node lifecycle and battery are all outside it."
