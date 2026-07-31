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
| presence-cell v1 (unsigned) | `8QoVUmp1jFtQ15UU8ejVBW6QitarLWyjkPQo9pVX9FFS` | superseded — placeholder signatures |
| **presence-cell v2 (signed, verifying)** | `BhoDpconffn4vLJPq4yBWtA4QguyZ856neMR4ddu3PMt` | cell 9q8yy epoch 20666; real ML-DSA-65 signature (3309 B); contract verifies every record in `validate_state`; node2 fetch 1.16 s, byte-identical, and the fetched bytes re-verify offline while REJECTING replay into another cell |

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

## Ecosystem adoption: freenet-scaffold + ghostkeys (2026-07-31)

Reviewed both repos for reusable capability. Both paid off; one found a bug.

### freenet-scaffold (LGPL-3.0, v0.2.2) — ADOPTED

`ComposableState` is the ecosystem's CRDT interface: `verify / summarize /
delta -> Option<Delta> / apply_delta / merge`, all threaded with a
`parent_state` so a sub-state can validate against sibling fields.
`lkng-presence` had independently converged on the same five methods, so
`impl ComposableState for CellState` was mechanical. Worth having because:

- `CellState` can now be nested in a larger state by the `#[composable]`
  macro (how River composes `ChatRoomStateV1`).
- `freenet_scaffold::convergence` ships a `ConvergenceTestHarness` that
  permutes operations and checks commutativity/idempotency/associativity —
  a stronger version of LKNG's hand-rolled property test, and the natural
  home for future contracts' convergence tests.
- `ParentState = Self` today (presence is top-level); that single line is
  what changes if presence is ever nested.

Note the inherent `apply_delta` was renamed `apply_records` — same name as
the trait method shadowed it at call sites.

### ghostkeys (NO LICENSE — patterns only, code not copyable) — FOUND A BUG

The delegate's authorization model is worth copying wholesale later:
`SignatureRequestor` is **runtime-attested** (`WebApp(ContractInstanceId)` |
`Delegate(DelegateKey)`), and scopes are least-privilege — third-party apps
that request access get only `{ReadPublic, Sign}`, never `Export`, `Delete`
or `Admin`, which stay with the vault.

The load-bearing idea is `ScopedPayload`: *"The raw payload is never signed
alone — always wrapped with the attested caller identity."*

**Applying that lens to LKNG's own presence record exposed a real
vulnerability in code written the same day.** `PresenceRecord` carries no
cell or epoch — those are contract *parameters*. Signing the record alone
meant a validly-signed tile could be **lifted out of one cell and replayed
into any other cell or epoch**, fabricating presence anywhere on earth from
a single honest record. That is a location-spoofing primitive in a
location-privacy app.

Fix: `PresenceRecord::signing_payload(&CellParams)` binds
`(domain_tag, schema_v, cell_id, epoch)` into the signed bytes, so a
signature is valid only in the contract it was minted for. The domain tag
additionally prevents a signature over another LKNG structure from being
reinterpreted as a presence record. Three regression tests cover it;
`CellParams` moved into `lkng-presence` because it is now a security input,
not a contract-shell detail.

Also adopted from ghostkeys: `fingerprint()` = first 8 bytes of
BLAKE3(verifying key), base58 — a shareable-handle scheme (complements
Delta's 10-char prefix) for LKNG profile handles later. And the reminder
that **delegates need migration too**, not just contracts: replacing a
delegate's storage schema changes its WASM hash and therefore its
`DelegateKey`, requiring `legacy_delegates.toml`-driven re-import.

## Finding: self-contained records nearly broke pseudonym rotation (2026-07-31)

Adding the inline `verifying_key` to presence records (for River-#145
self-containment) quietly created a linkability hole large enough to
nullify PLAN.md's "durable profile revealed only after mutual
interaction":

The key travels in the tile, so it is public to anyone scraping a cell. If
tiles were signed by the durable identity, a scraper could lift the key
from a tile, derive the owner's profile address, and fetch the full
profile — and every tile that user ever posted would be permanently
linkable to one person across all cells and epochs. Rotation would have
been decorative.

**Fix: per-epoch subkeys.** `Identity::for_epoch(epoch)` derives a
throwaway signing identity via `BLAKE3::keyed_hash(master_seed,
"lkng/epoch-key/v1" || epoch)`. Because that is a PRF, holding one epoch
key reveals nothing about any other epoch key or about the master seed;
the owner can always re-derive any epoch's key (so tiles stay updatable,
and recovery from a backup restores that ability). `sign_presence` derives
the subkey from `params.epoch` internally — there is deliberately **no API
that signs a tile with the durable key**, so it cannot happen by accident.

Enforced by tests (`presence_never_exposes_the_durable_key`,
`epoch_keys_are_unlinkable_across_epochs`) and by
`examples/audit_privacy.rs`, which scans **real network-fetched bytes** and
asserts the durable key appears nowhere in them.

Live: cell 9q8yy epoch 20667 =
`E4SWQ7188dkvuohrfTfjohn5YLQyoHC4j6utj85j2xgj`. Durable handle
`F5MjsxWX34C` never appears on the network; the epoch handle
`bWDmh9wqYw5` does. Epoch 20666 kept the older master-signed tile, which
is also a live demonstration that epoch rollover produces a genuinely
separate contract.
