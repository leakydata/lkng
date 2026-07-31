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

probe() { # $1=id $2=label
  local code_time
  code_time=$(curl -s -o /dev/null -w "%{http_code},%{time_total}" \
    --max-time 120 "$NODE/v1/contract/web/$1/") || code_time="000,120"
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ),$1,$2,$code_time" >>"$OUT"
}

echo "gate1-probe: every ${INTERVAL}s -> $OUT (ctrl-c to stop)"
while true; do
  probe "$BASELINE_ID" baseline_popular
  [ -n "$EXTRA_ID" ] && probe "$EXTRA_ID" fresh_zero_subscriber
  sleep "$INTERVAL"
done
