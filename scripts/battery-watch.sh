#!/usr/bin/env bash
#
# Sample what the LKNG node costs a phone, over hours.
#
# ## Why this is a script and not a spot check
#
# The README has been claiming "short runs look fine; that is not the same
# claim" since the node first ran. It is the honest thing to say and it is
# also an admission that the most important number about a background P2P
# node on someone's phone — what it does to their battery overnight — has
# never been measured.
#
# Doze is the specific reason a spot check is worthless. Android
# progressively restricts background work the longer a device is idle, so
# the first ten minutes and the fifth hour behave differently by design.
# Only a long idle run shows whether the node keeps working under Doze, and
# what it costs while doing so.
#
# ## Why sampling is passive
#
# Every read here comes from dumpsys and /proc. Nothing wakes the screen,
# starts an activity, or touches the app — a measurement that perturbs Doze
# is a measurement of the wrong thing.
#
# Usage: scripts/battery-watch.sh [interval_seconds] [output.tsv]
set -uo pipefail

INTERVAL="${1:-300}"
OUT="${2:-/tmp/lkng-battery.tsv}"

if ! adb get-state >/dev/null 2>&1; then
  echo "no device" >&2
  exit 1
fi

# Header, tab-separated so it drops straight into any analysis.
if [ ! -s "$OUT" ]; then
  printf 'unix\tlevel\ttemp_c\tplugged\twakefulness\tnode_pids\tnode_rss_kb\tnode_cpu_ticks\n' > "$OUT"
fi

echo "sampling every ${INTERVAL}s into $OUT"
echo "leave the phone alone -- unplugged, screen off -- or the numbers mean nothing."

while true; do
  NOW=$(date +%s)

  DUMP=$(adb shell dumpsys battery 2>/dev/null)
  LEVEL=$(echo "$DUMP" | grep -oE '^\s*level: [0-9]+' | grep -oE '[0-9]+' | head -1)
  TEMP=$(echo "$DUMP" | grep -oE 'temperature: [0-9]+' | grep -oE '[0-9]+' | head -1)
  # dumpsys reports tenths of a degree.
  TEMP_C=$(awk -v t="${TEMP:-0}" 'BEGIN { printf "%.1f", t/10 }')
  # "powered" is any source; a plugged phone tells you nothing about drain.
  PLUGGED=$(echo "$DUMP" | grep -cE '(AC|USB|Wireless) powered: true')

  WAKE=$(adb shell dumpsys power 2>/dev/null | grep -oE 'mWakefulness=\w+' | head -1 | cut -d= -f2)

  # The node runs as more than one process; sum them.
  PIDS=$(adb shell "ps -A -o PID,RSS,ARGS 2>/dev/null | grep libfreenet | grep -v grep" 2>/dev/null)
  COUNT=$(echo "$PIDS" | grep -c . )
  RSS=$(echo "$PIDS" | awk '{s += $2} END { print s+0 }')

  # utime+stime from /proc, so CPU can be differenced between samples --
  # an absolute reading at one instant says nothing about cost over time.
  TICKS=0
  for pid in $(echo "$PIDS" | awk '{print $1}'); do
    T=$(adb shell "cat /proc/$pid/stat 2>/dev/null" 2>/dev/null \
        | awk '{ print $14 + $15 }')
    TICKS=$(( TICKS + ${T:-0} ))
  done

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$NOW" "${LEVEL:-}" "$TEMP_C" "${PLUGGED:-0}" "${WAKE:-}" \
    "${COUNT:-0}" "${RSS:-0}" "$TICKS" >> "$OUT"

  sleep "$INTERVAL"
done
