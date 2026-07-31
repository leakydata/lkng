# Phase 0 gate status

Live log of the three go/no-go gates from PLAN.md. Update as evidence lands.

## Gate 1 — Eviction (do fresh contracts survive?)

**Setup (2026-07-31):**

- Published `hello-lkng` (grow-set contract, 34-byte state) to mainnet:
  `DcxbCZAiajvVpDk2dKmAVCCCiopVJ23TQoWqqkw1DaPT`, from the resident node
  (`:7509`). Round-trip verified byte-identical at publish time.
- Second node ("node2", `:7510`, own config/data dirs, **not** the
  publisher, no subscription) launched as the honest observer.
- `scripts/gate1-probe.sh` logs three series every 30 min to
  `tests/simulation/gate1-probe.csv`:

| Series | Via | Measures |
| --- | --- | --- |
| `baseline_popular` | publisher node HTTP | control: many-subscriber contract (Delta UI) |
| `fresh_zero_subscriber` | publisher node (`:7509`) | local retention on the publishing peer |
| `fresh_from_node2` | node2 (`:7510`) | **true network retention — the Gate 1 number** |

**First samples:** all three series `200`; node2 fetched the fresh contract
from the network in **0.83 s** cold, ~0.08 s warm. A zero-subscriber,
zero-demand contract is retrievable across the network today.

**What is NOT yet answered:** survival over days as hosting budgets fill and
demand-ordered eviction bites (the design doc's whole point), and survival
while the *publisher is offline* — the real dating-app scenario. Next steps:
let the series accumulate ≥1 week; then stop the publisher node for 24 h and
watch `fresh_from_node2`; then repeat during higher network load.

**Verdict: OPEN — early signal positive.**

## Gate 2 — Android node

- **CROSS-COMPILE PASSES (2026-07-31): unmodified freenet-core 0.2.116
  builds for `aarch64-linux-android` with ZERO code changes.** 4m46s build,
  52 MB unstripped → **36 MB stripped** ELF PIE. No patches, no upstream
  work needed for compilation — the plan's "may require upstream work"
  risk did not materialize at this stage.
- Recipe: `freenet-core` pins Rust 1.94.0 (`rust-toolchain.toml`); the
  pinned toolchain needs its own Android std
  (`rustup target add aarch64-linux-android --toolchain 1.94.0`) — without
  it every crate fails with a missing-std cascade. Then NDK r27c clang as
  linker/CC/CXX (`aarch64-linux-android24-clang`), `llvm-ar` as AR.
- Remaining half of the gate: run it **on a device** — child process from
  an APK, app-private dirs, loopback bind, survive `am kill`. Needs
  hardware or an emulator (and an x86_64 build for the latter).
- **fdev bug found (affects any contract dev):** `fdev build` panics
  "Could not find workspace root" unless `CARGO_TARGET_DIR` is set — its
  fallback walks the *compile-time* `CARGO_MANIFEST_DIR` of fdev itself
  (`crates/fdev/src/util.rs:103`). Also: contracts must be their own
  workspace root (`[workspace]` in the contract's Cargo.toml), matching
  Mail/Delta convention. Worth an upstream issue.

**ON-DEVICE RUN PASSES (2026-07-31).** Samsung Galaxy Z Flip 4
(SM-F721U1), **Android 16**, arm64-v8a. Unmodified freenet-core 0.2.116,
pushed to `/data/local/tmp/lkng`, run via adb shell:

- Executes and starts on Android 16 with **zero code changes**.
- **Joined the real network and acted as a full relay** — the log shows it
  processing `SUBSCRIBE relay` requests for other peers (e.g. upstream
  `45.249.164.109`) and sweeping idle streaming handles. Not a leaf: it
  carried other people's traffic unprompted.
- 201 peer-address log events in ~4 min; 28 MB data dir after ~7 min.
- **Multi-process:** the node runs as 2 PIDs. An Android foreground service
  must manage the **process group**, not a single PID — killing the parent
  left a child alive and still networking. This is a concrete design
  requirement for Phase 5, found early.
- Survived `pkill -9` of all processes and **cold-restarted from the same
  data dir** (new pid), no corruption or load errors in the log.

Not yet measured: battery drain over hours, cellular-vs-Wi-Fi behaviour,
and behaviour under Android's Doze/App Standby (all Phase 5/Gate 3 work,
and all require an APK rather than adb shell).

**Verdict: PASS for build + run. Battery/Doze behaviour still open.**

## Gate 3 — Duty-cycled contribution

Not started. Depends on Gate 2 binary. Plan: drive node between
contributing and leaf across charge/network transitions on a real device;
watch ring stability and state integrity.

**Verdict: NOT STARTED.**

## Environment notes

- Resident node `:7509` (network mode) — also serves River/Delta locally.
- node2 `:7510` / network port 31338, dirs under `~/.cache/lkng-node2/`.
- Toolchain: Rust 1.97.1 (workspace) + 1.94.0 (freenet-core pin),
  stdlib 0.8.5, fdev 0.3.278, freenet 0.2.116, NDK r27c, JDK 21+25.

## Live contracts (mainnet)

| Contract | ID | Notes |
| --- | --- | --- |
| hello-lkng | `DcxbCZAiajvVpDk2dKmAVCCCiopVJ23TQoWqqkw1DaPT` | pipeline probe, Gate 1 series |
| presence-cell (9q8yy, epoch 20666) | `8QoVUmp1jFtQ15UU8ejVBW6QitarLWyjkPQo9pVX9FFS` | first real grid cell; genesis record "first tile in the grid"; node2 fetch 0.7 s |

## Finding: UPDATE to cold contracts fails (2026-07-31)

`fdev execute update` (both `Delta` and `--as-state`) against the freshly
published presence cell fails with `UPDATE failed: missing contract` — from
the publisher node AND node2 — while GET succeeds from both in <1 s.
Re-publishing with `--subscribe` doesn't change it. Reads propagate; the
update op appears to route to ring-location hosts that don't hold the
contract. River works subscription-first (subscribe → delta mesh → pushes),
which suggests the reliable write path is: subscribe, wait for the mesh,
then update — not fire-and-forget updates at a cold contract.

Implications for LKNG: presence publishing must be subscription-first (the
app subscribes to its own cell anyway, so this matches the design), and this
needs an upstream issue with a minimal repro. Repro artifacts:
`contracts/presence-cell/{state2.bin,delta.bin}` generators in `examples/`.

## Gate 2 addendum

x86_64-linux-android also builds clean (4m54s, 55 MB unstripped) — both
ABIs needed for emulator + device testing now exist.
