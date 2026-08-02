# `summarize_contract_state` is invoked ~89×/second per contract on an idle node

**Status:** draft, not yet filed. Needs a minimal reproduction against a
stock node before it is worth anyone's time upstream.

## What we measured

A phone running an unmodified `freenet-core` node — screen off, device in
Doze, app in the background, no user activity — sustained **~41% of one CPU
core** over 30 minutes of five-minute samples (`/proc/<pid>/stat`, utime +
stime differenced between samples).

The node's own rate-limited logging points at the cause:

```text
[RATE LIMIT per-callsite] summarize_contract_state: dropped 1780 in last 30s
                                                   (cumulative: 409369)
```

At the documented 30/sec per-callsite cap, 1 780 dropped lines per 30 s
implies roughly **89 calls per second**, sustained, indefinitely.

## Why that is expensive

`summarize_state` is a WASM entry point. Each call deserialises contract
state inside the sandbox. For any contract whose state is more than trivial
— ours holds up to 500 records with a 16 KiB image each — that is megabytes
of deserialisation per second whose result is discarded except for a set of
keys.

We reduced our own cost substantially by decoding only the map keys and
skipping values with `serde::de::IgnoredAny`, which avoids materialising the
images. That is a fix every contract author would have to discover
independently, and it does not address the call rate itself.

## Questions for maintainers

1. Is ~89 calls/sec/contract on an idle node the intended cadence, or is
   something re-triggering summarisation in a loop?
2. Could summaries be cached and invalidated on state change? The state of
   an idle contract does not change between calls, so almost all of this
   work is recomputing an identical answer.
3. If the rate is intended, it is worth documenting prominently that
   `summarize_state` is the hottest path in a contract and must be O(keys)
   rather than O(state). We wrote a naive implementation and it was the
   dominant cost on a phone.

## Why this matters for mobile

This is the difference between a Freenet node being viable on a phone and
not. 41% of a core sustained is a background service users uninstall. The
same node on a desktop would go unnoticed, which is likely why it has not
surfaced before.

## Environment

- `freenet-core`, `fdev` 0.3.278, Rust 1.94.0
- Samsung Galaxy Z Flip 4, Android 16, aarch64-linux-android
- Node as a child process of an ordinary app, foreground service
- Also reproduced on x86_64 Linux (same log signature, CPU cost not isolated)
