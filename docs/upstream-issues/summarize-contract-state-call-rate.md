# Idle CPU scales with hosted contracts; `summarize_contract_state` called ~89×/s

**Status:** draft. Reproduction script included and run;
[`scripts/repro-idle-cpu.sh`](../../scripts/repro-idle-cpu.sh).

## Summary

An unmodified `freenet-core` node running on an Android phone consumes
**~50% of one CPU core, sustained and indefinitely, with no client
connected**. On a desktop this is invisible. On a phone it is the difference
between a viable background service and one users uninstall.

## Measurement

Samsung Galaxy Z Flip 4, Android 16, `aarch64-linux-android`. Node runs as a
child process of an ordinary app under a foreground service. Sampling is
passive (`/proc/<pid>/stat`, utime+stime differenced between 5-minute
samples) so it does not perturb Doze.

Conditions during measurement:

- screen off, device in Doze (`mWakefulness=Dozing`)
- **no WebSocket client connected** — verified via `/proc/net/tcp`; the app's
  WebView was not running, only the service
- device otherwise idle, on charge

Over 100 minutes of 5-minute samples, CPU stayed between **47% and 61% of
one core**, mean ≈ 51%. It does not decay: the lowest readings are as late
as the highest.

```text
00:45   49.3% of one core    rss 371 MB
00:55   47.8%                rss 325 MB
01:05   47.2%                rss 326 MB
01:15   53.9%                rss 340 MB
```

Work is spread across ~8 `freenet-main` tokio worker threads (15 threads
total), so it is not one runaway task.

**Memory is fine and was checked separately.** RSS tracks hosted contracts
and is released: it climbed 275 → 614 MB under heavy contract publishing and
returned to ~365 MB within 20 minutes of going idle. No ratchet.

## The signal that points somewhere

The node's own rate-limited logging:

```text
[RATE LIMIT per-callsite] summarize_contract_state: dropped 1780 in last 30s
                                                   (cumulative: 409369)
```

At the 30/sec per-callsite cap, 1 780 dropped lines per 30 s implies
**~89 calls/second**, sustained, on an idle node with no client. Each call
enters WASM and deserialises contract state.

By comparison, over the same period: `peer_connection` 632 log events,
`process_network_message` 151, `fetch_contract` 6.

## What we ruled out on our side

- **Our contract's `summarize_state` was needlessly expensive** — it decoded
  the whole state (up to 500 records with a 16 KiB image each) to collect map
  keys. We now decode keys and skip values with `serde::de::IgnoredAny`.
  **This did not measurably change the CPU.**
- **Subscription count** — we halved our watch set from 18 contracts to 10.
  **No measurable change**, which is consistent with the app not being
  connected during measurement.
- **Client activity** — none. No WebSocket connection existed.

## The reproduction: cost scales with hosted contracts

`scripts/repro-idle-cpu.sh` starts a **fresh node** — empty data directory,
no contracts, no clients — lets it join the ring for 90 s, then samples CPU
identically to everything above. On the same machine, at the same moment, it
also samples the long-running node that hosts many contracts:

```text
empty data dir, 0 contracts, 0 clients:   0.4% of one core,   42 MB
same contracts, 2 min after restart:      7.1% of one core, 1040 MB
same contracts, 5 h uptime:              27.4% of one core, 1192 MB
```

Two things, not one:

1. **A node with nothing to host costs essentially nothing** (0.4%), and it
   logs **zero** `summarize_contract_state` lines. So the idle cost is not a
   fixed baseline — it needs contracts to exist.
2. **But it also grows with uptime on an unchanged contract set**: the same
   node, same data directory, went 7.1% → 27.4% over five hours. Contract
   count alone does not explain that.

The second point is the more interesting one and we cannot explain it. The
contract set did not grow appreciably over those five hours; the machine was
otherwise idle for much of it. Something accumulates.

That makes the mechanism concrete: the per-contract idle cost, multiplied by
contract count, is what consumes half a phone core. It also suggests the fix
is tractable — an idle contract's state does not change between calls, so
almost all of that ~89 calls/second is recomputing an identical summary.

## A desktop comparison, which complicates the story

The same node software on x86_64 (Xeon E5-2630 v4, 2.2–3.1 GHz), same
network, uses **18.5% of one core** — measured over 60 s, 4h20m uptime.

So the node is probably doing a roughly *constant* amount of background
work, and the difference is core throughput: ~18% of a desktop core and
~50% of a phone core (2.02 GHz little core, far lower IPC). That is a
meaningfully weaker claim than "phone-specific bug", and it is the honest
reading of the two numbers.

It does not make the problem go away. A constant background cost that
consumes half a phone core is still the thing that decides whether a node
can live on a phone, and the phone is where the constraint bites. But a
maintainer should know that this looks like *the node's normal idle cost
measured on weaker hardware*, not a mobile-only defect.

The desktop figures are otherwise not representative of a user: that node
carries 1.2 GB RSS and 7 leaked client WebSocket connections after an
evening of development, which is a dev artefact rather than something a
user's node would accumulate.

## Questions

1. Is ~89 calls/s the intended summarisation cadence for an idle node with
   no client connected?
2. **Why does CPU grow ~4× over five hours on an unchanged contract set?**
   That is the part we cannot account for, and it is the one that decides
   whether a phone node is viable over a day rather than an hour.
3. Can summaries be cached and invalidated on state change? An idle
   contract's state does not change between calls, so nearly all of this
   work recomputes an identical answer.
4. If the cadence is intended, it deserves prominent documentation:
   `summarize_state` is by far the hottest path in a contract and must be
   O(keys), not O(state). We wrote the obvious implementation and it was
   costly.

## What would make this a better report

A minimal reproduction: a stock node hosting one trivial contract, no app,
CPU sampled the same way. We have not built that yet, and it is the first
thing a maintainer would ask for.

## Environment

- `freenet-core` (unmodified), `fdev` 0.3.278, Rust 1.94.0 (pinned upstream)
- Android 16, aarch64; also observed the same log signature on x86_64 Linux,
  where CPU cost was not isolated
