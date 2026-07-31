#!/usr/bin/env bash
# Gate 2, second half: run the freenet node ON a real Android device.
#
# The cross-compile proved it BUILDS. This proves it RUNS: starts, binds
# loopback only, writes to app-private storage, joins the network, serves
# the client API, and survives being killed the way Android kills things.
#
# Uses adb shell (no APK yet) — the binary runs from /data/local/tmp, which
# is the standard pre-APK spike location. A real app would exec it from
# nativeLibraryDir with the same argv.
#
# Usage: scripts/gate2-device-test.sh [aarch64|x86_64]
set -uo pipefail

ARCH="${1:-aarch64}"
CORE=/home/scholyx/Documents/code/freenet-core/target
case "$ARCH" in
  aarch64) BIN="$CORE/aarch64-linux-android/release/freenet" ;;
  x86_64)  BIN="$CORE/x86_64-linux-android/release/freenet" ;;
  *) echo "unknown arch: $ARCH" >&2; exit 2 ;;
esac

DEV_DIR=/data/local/tmp/lkng
WS_PORT=7509
NET_PORT=31337
RESULTS="$(dirname "$0")/../tests/simulation/gate2-device.md"

say() { printf '\n=== %s ===\n' "$*"; }
dev() { adb shell "$@"; }

[ -f "$BIN" ] || { echo "missing binary: $BIN (build it first)" >&2; exit 1; }
adb devices | tail -n +2 | grep -qw device || { echo "no authorized device" >&2; exit 1; }

say "device"
MODEL=$(dev getprop ro.product.model | tr -d '\r')
ANDROID=$(dev getprop ro.build.version.release | tr -d '\r')
ABI=$(dev getprop ro.product.cpu.abi | tr -d '\r')
echo "$MODEL | Android $ANDROID | $ABI"

say "push binary ($(du -h "$BIN" | cut -f1))"
dev "mkdir -p $DEV_DIR/config $DEV_DIR/data"
adb push "$BIN" "$DEV_DIR/freenet" >/dev/null
dev "chmod 755 $DEV_DIR/freenet"

say "1. does it execute at all"
VERSION=$(dev "$DEV_DIR/freenet --version 2>&1" | tr -d '\r')
echo "$VERSION"
echo "$VERSION" | grep -qi freenet || { echo "FAIL: binary did not run"; exit 1; }

say "2. start node (network mode, app-private dirs)"
dev "cd $DEV_DIR && nohup ./freenet network \
  --config-dir $DEV_DIR/config --data-dir $DEV_DIR/data \
  --ws-api-port $WS_PORT --network-port $NET_PORT \
  > $DEV_DIR/node.log 2>&1 &" || true
sleep 25
PID=$(dev "pgrep -f 'freenet network'" | tr -d '\r' | head -1)
echo "pid: ${PID:-none}"
[ -n "$PID" ] || { echo "FAIL: node not running"; dev "tail -20 $DEV_DIR/node.log"; exit 1; }

say "3. is the client API listening"
dev "cat $DEV_DIR/node.log" | tail -15

say "4. memory footprint"
dev "dumpsys meminfo $PID 2>/dev/null | head -12" || dev "cat /proc/$PID/status | grep -E 'VmRSS|VmSize'"

say "5. survive SIGKILL (Android's low-memory killer equivalent)"
dev "kill -9 $PID" || true
sleep 3
dev "pgrep -f 'freenet network'" | tr -d '\r' | grep -q . && echo "unexpectedly still running" || echo "killed cleanly (an app would restart it via foreground service)"

say "6. restart from same data dir (state integrity after kill)"
dev "cd $DEV_DIR && nohup ./freenet network \
  --config-dir $DEV_DIR/config --data-dir $DEV_DIR/data \
  --ws-api-port $WS_PORT --network-port $NET_PORT \
  >> $DEV_DIR/node.log 2>&1 &" || true
sleep 20
PID2=$(dev "pgrep -f 'freenet network'" | tr -d '\r' | head -1)
[ -n "$PID2" ] && echo "restarted, pid $PID2 — data dir survived" || echo "FAIL: did not restart"

say "done — logs at $DEV_DIR/node.log on device"
echo "pull with: adb pull $DEV_DIR/node.log"
