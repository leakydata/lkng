#!/usr/bin/env bash
# Gate 1 (eviction) probe — phase A: baseline.
#
# Measures GET availability/latency of a KNOWN-POPULAR contract (Delta's
# published web container) through the local node, on an interval, into a
# CSV. This is the control series: a many-subscriber contract that should
# essentially never be unfetchable.
#
# Phase B (after the hello-world contract exists): publish a FRESH contract
# with zero subscribers from a second node, take that node offline, and run
# this same probe against it. Comparing the two series answers Gate 1:
# does a new user's single-subscriber contract survive, and for how long?
#
# Usage: scripts/gate1-probe.sh [interval_seconds] [extra_contract_id]
set -u

INTERVAL="${1:-1800}"
NODE="http://127.0.0.1:7509"
BASELINE_ID="EqJ5YpEEV3XLqEvKWLQHFhGAac2qXzSUoE6k2zbdnXBr" # Delta UI container
EXTRA_ID="${2:-}"
OUT_DIR="$(dirname "$0")/../tests/simulation"
OUT="$OUT_DIR/gate1-probe.csv"

mkdir -p "$OUT_DIR"
[ -f "$OUT" ] || echo "utc_iso,contract,label,http_code,time_total_s" >"$OUT"

probe_web() { # $1=id $2=label — webapp contracts via HTTP
  local code_time
  code_time=$(curl -s -o /dev/null -w "%{http_code},%{time_total}" \
    --max-time 120 "$NODE/v1/contract/web/$1/") || code_time="000,120"
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ),$1,$2,$code_time" >>"$OUT"
}

probe_state() { # $1=id $2=label $3=ws_port — raw contracts via the client API
  local start elapsed code
  start=$(date +%s.%N)
  if "$HOME/.local/bin/fdev" -p "$3" execute get "$1" -o /dev/null --timeout 120 >/dev/null 2>&1; then
    code=200
  else
    code=000
  fi
  elapsed=$(echo "$(date +%s.%N) - $start" | bc)
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ),$1,$2,$code,$elapsed" >>"$OUT"
}

echo "gate1-probe: every ${INTERVAL}s -> $OUT (ctrl-c to stop)"
while true; do
  probe_web "$BASELINE_ID" baseline_popular
  if [ -n "$EXTRA_ID" ]; then
    # Series 2: via the PUBLISHER node (measures local retention).
    probe_state "$EXTRA_ID" fresh_zero_subscriber 7509
    # Series 3: via a NON-publisher node on :7510 (measures true network
    # retention — this is the number Gate 1 actually turns on). Skipped
    # silently if node2 isn't running.
    ss -tln 2>/dev/null | grep -q ':7510 ' &&
      probe_state "$EXTRA_ID" fresh_from_node2 7510
  fi
  sleep "$INTERVAL"
done
