#!/usr/bin/env bash
#
# Minimal reproduction for the idle-node CPU report.
#
# Starts a **fresh** node with an empty data directory, no contracts and no
# clients, and samples its CPU the same way the phone was sampled. Then
# compares against the long-running node on this machine, which hosts many
# contracts.
#
# The question it answers is the one a maintainer would ask first: is the
# idle cost a constant baseline, or does it scale with hosted contracts?
# Everything measured so far has been on nodes that host plenty, so the
# answer is currently unknown — and "unknown" is the honest state of the
# report until this runs.
#
# Usage: scripts/repro-idle-cpu.sh [seconds]
set -uo pipefail

SECS="${1:-180}"
DIR=$(mktemp -d /tmp/freenet-repro.XXXXXX)
WS_PORT=7611
NET_PORT=31411

cpu_pct() {
  # utime+stime from /proc, differenced. Same method as the phone sampling,
  # so the numbers are comparable rather than merely similar.
  local pid="$1" secs="$2" t1 t2
  t1=$(awk '{print $14 + $15}' "/proc/$pid/stat" 2>/dev/null) || return 1
  sleep "$secs"
  t2=$(awk '{print $14 + $15}' "/proc/$pid/stat" 2>/dev/null) || return 1
  awk -v t=$((t2 - t1)) -v s="$secs" 'BEGIN { printf "%.1f", (t/100.0)/s*100 }'
}

rss_mb() {
  awk '/VmRSS/ { printf "%d", $2/1024 }' "/proc/$1/status" 2>/dev/null
}

echo "==> starting a fresh node (empty data dir, no contracts, no clients)"
echo "    data dir: $DIR"
# The node expects these to exist rather than creating them, and the error
# ("Configuration directory not found") does not say which one is missing.
mkdir -p "$DIR/config" "$DIR/data"
# A fresh config dir has no gateway list, and a node with nowhere to connect
# is not a node at rest -- it is a node failing to start. Reuse the working
# one, which is what a new install would receive anyway.
if [ -f "$HOME/.config/freenet/gateways.toml" ]; then
  cp "$HOME/.config/freenet/gateways.toml" "$DIR/config/gateways.toml"
fi
setsid nohup freenet network \
  --config-dir "$DIR/config" --data-dir "$DIR/data" \
  --ws-api-port "$WS_PORT" --network-port "$NET_PORT" \
  > "$DIR/node.log" 2>&1 < /dev/null &
sleep 3
FRESH=$(pgrep -f "ws-api-port $WS_PORT" | head -1)
if [ -z "$FRESH" ]; then
  echo "FAIL: fresh node did not start; see $DIR/node.log" >&2
  tail -5 "$DIR/node.log" >&2
  exit 1
fi

echo "    pid $FRESH — letting it join the ring for 90s before measuring,"
echo "    because a node still bootstrapping is not an idle node."
sleep 90

echo
echo "==> sampling for ${SECS}s"
FRESH_CPU=$(cpu_pct "$FRESH" "$SECS")
FRESH_RSS=$(rss_mb "$FRESH")

# The established node on this machine, for contrast.
BUSY=$(pgrep -f 'freenet network' | grep -v "^$FRESH$" | head -1)
BUSY_CPU=""
if [ -n "$BUSY" ]; then
  BUSY_CPU=$(cpu_pct "$BUSY" 30)
  BUSY_RSS=$(rss_mb "$BUSY")
fi

echo
echo "-------------------------------------------"
printf 'fresh node   (0 contracts, 0 clients):  %5s%% of one core, %s MB\n' \
  "$FRESH_CPU" "$FRESH_RSS"
if [ -n "$BUSY_CPU" ]; then
  printf 'established  (many contracts):          %5s%% of one core, %s MB\n' \
    "$BUSY_CPU" "$BUSY_RSS"
fi
echo
echo "summarize_contract_state rate-limit lines in the fresh node's log:"
grep -c 'summarize_contract_state' "$DIR/node.log" 2>/dev/null || echo 0

echo
echo "fresh node still running as pid $FRESH; kill it with:"
echo "  kill $FRESH && rm -rf $DIR"
