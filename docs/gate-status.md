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
| presence-cell v2 (signed, verifying) | `BhoDpconffn4vLJPq4yBWtA4QguyZ856neMR4ddu3PMt` | cell 9q8yy epoch 20666; real ML-DSA-65 signature (3309 B); contract verifies every record in `validate_state`; node2 fetch 1.16 s, byte-identical, and the fetched bytes re-verify offline while REJECTING replay into another cell |

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

## Profile contract live (2026-07-31)

`73s49yXNmtGsf9VQA9y9fFyqRK38PE2RzBYeGJVCgL35` — handle `F5MjsxWX34C`,
published and fetched byte-identical from the non-publisher node, then
re-verified offline from the network bytes.

Single-writer, Delta-shaped. The owner's durable verifying key is pinned in
the contract **parameters**, so the address *is* the identity: `hash(code,
params)` differs per owner and the contract rejects any state not signed by
that key. This closes Delta's address-claiming problem outright — there is
no first-write-wins race, because a squatter's write simply fails
verification (test: `another_identity_cannot_occupy_this_address`).

Delta lessons carried over and tested:

- per-item signatures, not whole-state
- monotonic sequence with a **content-hash tiebreak**, so equal-sequence
  conflicts resolve identically on every peer with no clock involved
- **signature schema evolution**: `verify_state` tries the v2 payload
  layout then falls back to v1, so adding a field later doesn't silently
  delete every profile already on the network
- **authenticated tombstones bound to their sequence**: retargeting a
  deletion breaks its signature (`forged_deletion_is_rejected`), and a
  deletion is sticky against replay of older bodies while still allowing
  the owner to re-publish with a higher sequence

Privacy separation is verified against real network bytes
(`examples/verify_profile_from_network.rs`): the profile contains **no
epoch key**, so holding someone's profile does not let you locate their
tiles — and the tiles carry no durable key, so scraping a cell does not
lead to the profile. The link exists in exactly one direction, only when
its owner chooses to prove it.

## RESOLVED: multi-writer contracts work — one session + seed PUT (2026-07-31)

The earlier "cold-contract UPDATE fails" note now has a root cause, and it
is worse than a papercut — it is an upstream regression that blocks LKNG's
core write path.

**Symptom.** `fdev execute update` fails `UPDATE failed: missing contract`
while `GET` on the same key from the same node returns full state in <1 s.

**Root cause.** The local node does not *host* the contract. `fdev
diagnostics` after a successful publish and a successful 5582-byte GET:

```
📄 Contract States:
| 6TUPTa5dCuCLBPjZGxuzW7BtYMDMD6BTChY13hcBE9sp | 0 | None |
📊 Hosting contracts: 214
```

214 contracts hosted, 54 connections, subscription registered — and 0/None
for the contract just published. `state_store.get(&key)` returns
`MissingContract` (`executor_impl.rs:697-707`), so `drive_client_update`
fails *before forwarding*. Per upstream #4071, the wire format carries a
post-merge **full state**, not a delta, so the originator must merge
locally first — which needs code and params it doesn't have.

**This is a regression.** #4066 and #4071 describe exactly this and were
both closed 2026-05-09; it reproduces on 0.2.116 (July 2026).

**Ruled out** (none of these change the outcome): publishing with and
without `--subscribe`; explicit `fdev execute subscribe` first (the
subscription *does* register); `get --return-code` first; `--as-state`
instead of a delta; a second independent node; immediate vs. delayed.

### What it means for LKNG

A node can only write to contracts it happens to host, and hosting follows
**ring location**, not user interest. So a phone can publish its *own*
contracts (profile, and the first tile in a cell) but cannot reliably
append to a **shared** per-area presence cell — which is the mechanism
discovery depends on.

Reads are unaffected and remain fast. What is blocked is many users
appending to one contract.

### Resolution — it was never a core bug, it was `fdev`

Reading River's `room_synchronizer.rs` gave the answer. River does not
merely subscribe before writing; on joining a room it issues a
`ContractRequest::Put` carrying the **contract code + parameters + state**
with `subscribe: true` — its own comments call this the "seed PUT" — and
it does so over a **single long-lived `WebApi` session** that then carries
every later GET, SUBSCRIBE and UPDATE.

`fdev` cannot reproduce that: **every `fdev execute …` invocation opens a
fresh WebSocket**. Publish in one process and update in the next, and the
second session has no seeded contract to merge against — hence
`missing contract`.

**Confirmed working.** `rust/lkng-transport-freenet` opens one session,
seed-PUTs, then updates:

```
connected (one session for everything)
seed PUT ok: 3fWWKKoRC9Y2JTtLcJ8cwT8mT5Gu7HGRsDGt1NtWhkSf
state before update: 5582 bytes
UPDATE ACCEPTED — state now 11157 bytes (was 5582)
>>> WRITE PATH WORKS: a second author's record merged in
```

Two independent identities now hold tiles in one shared cell. Fetched from
the **non-publisher** node: 11157 bytes, both records verify, and both
still reject replay into another cell. Repeat runs stay at 11157 bytes —
idempotent re-application, exactly as the grow-set requires.

**Implication for LKNG:** the phone client must hold one persistent
WebSocket to its local node for the app's lifetime (which the foreground
service was going to do anyway) and seed-PUT any cell before writing to
it. No architectural change, no coordinator, no fallback design needed.

The upstream draft is retargeted at the real defect: `fdev`'s
one-connection-per-invocation model makes multi-writer contracts appear
broken, and the error message points at the contract rather than the
session. Low severity now that the correct pattern is known — but it cost
a day, and the next person deserves better.

## Transport abstraction closed the loop (2026-07-31)

`rust/lkng-transport-freenet` now implements `lkng_transport::Transport`,
so application logic is genuinely backend-agnostic.
`examples/backend_agnostic.rs` defines one `publish_tile()` — sign a
record, publish, read back — and runs it unchanged against both backends:

```
mock backend:    published + read back 5564 bytes
freenet backend: published + read back 5568 bytes
```

The function never mentions sessions, seed PUTs or contract code. Three
design points made that possible:

- **Registration is mandatory, not an optimisation.** `register_contract(code,
  params)` returns the `StateKey` app code uses. An UPDATE needs a seed PUT
  carrying the code, and a `ContractKey` (instance id + code hash) cannot
  be rebuilt from an instance id alone — so the registry holds the derived
  key. Unregistered keys fail with *"call register_contract first"* rather
  than surfacing later as `missing contract`.
- **First publish seeds, later ones update.** The adapter tracks which keys
  it has seeded on this session and picks the right operation, so the
  distinction never reaches app code.
- **Signing is deliberately NOT implemented here.** `sign`/`verify` return
  an error pointing at `lkng-identity` and the contract verifiers. Routing
  signing through the transport would put key access on the network path —
  precisely the boundary the identity delegate exists to hold.

`subscribe` registers with the node (which is what keeps a contract hot)
but returns an empty stream for now: fanning update notifications out to
per-key streams needs a demultiplexer owning `recv`, which lands with the
UI work. Blob methods fail loudly pending Phase 4's ChunkedPack — a
plausible stub would be worse than an honest error.

## Live grid working: real-time notifications (2026-07-31)

`src/demux.rs` replaces the scan-until-you-find-your-reply approach with a
proper demultiplexer: one background task owns `recv()` forever and routes
each response either to a **oneshot** (a caller awaiting a specific reply,
matched on contract id *and* response kind) or to a **per-contract
broadcast** (subscribers). Requests go through the same task, so `WebApi`
has a single owner and no lock is held across an await on the socket — a
slow grid view cannot stall the reader.

`examples/live_grid.rs` subscribes to a cell, then writes a tile to it:

```
session up, reader demultiplexing
seeded + subscribed: 14c9xkKWeUu9skGwnbZ6ZFVGub1iwLJCtCLqqR29jPPA
>>> LIVE NOTIFICATION: 11157 bytes, 2 tile(s) in the grid
update accepted; waiting for the notification to land...
```

**Note the ordering.** The notification printed *before* the update
response — they genuinely interleave on the wire. The previous
`FreenetClient::await_response` would have discarded that notification
while scanning for its own reply, which is exactly the bug a naive client
ships with and never notices until the grid mysteriously fails to refresh.

Matching on `(contract_id, ReplyKind)` rather than contract alone also
matters: a PUT and a GET for the same contract can be in flight together.
Waiters are FIFO per key so two concurrent identical requests can't steal
each other's replies.

That is the last piece of plumbing before a grid can be drawn: tiles now
arrive pushed, not polled.

## END TO END: two strangers met over Freenet (2026-07-31)

`examples/two_strangers.rs` runs the entire product premise against the
live network. Verbatim output:

```
alex publishes cell 9q8yy
sam  publishes cell 9q8yy
same cell: true  (they can discover each other)

cell live: EyB2mhbXwyM6A8cJuyQEtzMHRbneSNZHRev4hNM83tD7
alex posted the first tile

alex's grid refreshed, live — 2 tiles:
  ✓ "alex: new here" (verified)
  ✓ "sam: also new here" (verified)

scraper's view: 2 tiles, durable identities present: false
  -> a scraper cannot reach either profile from these bytes

sam revealed a profile at 7c88KiJCWBLJo6qMmbejkZxm4GaiBuCMD5CApVi38GS2 (handle BJJR7iojHFC)
alex fetched and verified it: "sam" — revealed only after we matched
```

Every claim in PLAN.md's core loop is now demonstrated rather than
asserted:

- **Proximity without disclosure.** Two different true positions
  (37.7749,-122.4194 and 37.7761,-122.4180), each jittered by a stable
  per-user offset, land in the same level-5 cell `9q8yy`. Raw coordinates
  never leave `lkng-location`; the cell string is the only location-derived
  value that reaches the network, and no distance is ever computed.
- **Discovery is live.** Alex's grid refreshed by push when sam's tile
  merged — no polling.
- **Every tile is verified**, in-contract and again by the client.
- **The scraper check is empirical.** Scanning the actual bytes that
  crossed the wire for either durable verifying key: absent. Harvesting
  this cell yields two epoch pseudonyms and no route to either profile.
- **Revelation is a choice.** Sam's profile only became reachable when sam
  published it, and alex could only verify it holding sam's durable key —
  obtained through the match, not through scraping.

That is the privacy property the whole design exists for, working on
mainnet, with no server in the path.

## Frontend decision: Dioxus (Rust → WASM), not TypeScript

Phase 0 left this open. The deciding argument is not taste, it is
**avoiding a second implementation of security-critical code**.

The client must sign presence records (ML-DSA-65 over a canonically
encoded, domain-separated payload), verify every tile it renders, verify
fetched profiles, and derive per-epoch subkeys. All of that already exists,
tested, in `lkng-identity` / `lkng-presence` / `lkng-profile`.

With a TypeScript UI, every one of those would need a JavaScript
reimplementation — ML-DSA, canonical CBOR, the exact signing-payload
layout, the epoch derivation. Any divergence between the two produces
records that verify on one side and not the other, which is precisely the
failure mode we avoided earlier by making the contract and the client share
one verifier. Shipping a second signing stack in a different language would
undo that on purpose.

Dioxus compiles the existing crates straight into the UI. `getrandom`
works on `wasm32-unknown-unknown` in a browser via its `js` feature (the
contract target, which has no JS, is why in-contract verification is
feature-gated to be RNG-free — different target, different constraint).

Delta already ships a Dioxus UI against `freenet_stdlib::client_api::WebApi`,
so the pattern is proven in the ecosystem.

## Grid UI running (2026-07-31)

`web/` is a Dioxus app compiled to WASM, rendering tiles produced by
`lkng-app`. Every tile in the demo grid is **genuinely signed and genuinely
verified** by the same code path the network uses — only delivery is
faked, so the interface can be developed with no node, no account and no
network. Pointing it at `lkng-transport-freenet` swaps the data source and
nothing else.

Layout: **3 columns on phones, 4 above 560px**. Fixed counts rather than
`auto-fill`, because the grid has to read as a wall of faces at a glance
and a tile that stretches to 300px on a wide screen stops feeling like one.

Three build notes worth keeping:

- **Three `getrandom` majors reach a browser build** — 0.4 and 0.3
  directly, 0.2 transitively via argon2/chacha20poly1305 on rand_core 0.6.
  Each needs its own browser-backend feature (`wasm_js`, `wasm_js`, `js`)
  or the wasm target refuses to compile. This is invisible until you try.
- **`base_path` in Dioxus.toml 404s the root** under `dx serve`; remove it.
- **A real overflow bug shipped to the browser and panicked the app**:
  `u16::from(byte) * 360` overflows (255 × 360 = 91,800 > 65,535). It
  panicked in debug — which is the good outcome; in release it would have
  silently wrapped and produced wrong colours nobody would have traced.
  Running the thing in an actual browser found it in one load.

## Encrypted messaging live on mainnet (2026-07-31)

Bob's inbox: `4rTB68B6U1MepZnVcdjWMYTdNz9T8D2cNXFrgzi3BP5M`. Alice sealed a
message to it; the state was fetched back from the **non-publisher** node
and read:

```
1 pending message(s) in bob's inbox
  from epoch-key QprPyeCP…: "saw your tile - fancy a coffee?"
no stranger can decrypt; plaintext absent from the published bytes
```

### A broken design caught before it shipped

The first attempt derived the message key symmetrically from each side's
own seed — which cannot work, because **ML-DSA is a signature scheme with
no key agreement**. Sender and recipient would have derived different keys.
It compiled cleanly; only writing the round-trip test would have caught it.

Replaced with proper **ECIES**, the construction River uses: a fresh
ephemeral X25519 keypair per message, ECDH against the recipient's static
encryption key, HKDF-SHA256 to a symmetric key (binding both public keys so
a shared secret can never be reused in another context), then
XChaCha20-Poly1305. The ephemeral public key rides in the envelope.

Identities now carry **two** keypairs from one seed: ML-DSA-65 for
signing, X25519 for encryption. One 32-byte backup still restores both.

### Properties tested rather than asserted

- an envelope **binds its recipient**, so it cannot be lifted into another
  inbox to forge "they messaged you" — the same replay class as the
  presence records, caught in a second place
- signature verified **before** decryption, so a forged sender's bytes
  never reach the cipher
- the envelope carries the sender's **epoch** key, not their durable one:
  messaging does not undo what epoch subkeys bought. The recipient can
  still tie the message to the tile they tapped.
- identical plaintexts produce distinct ciphertexts (no nonce or ephemeral
  reuse)
- the **processed-set is owner-signed**: nobody else can mark your inbox
  read and hide messages from you. Processed ids *union* on merge, so two
  devices reading different messages converge to "both read" instead of
  one erasing the other.
- caps everywhere, because an open inbox with an unbounded collection is a
  free denial-of-service on the recipient's bandwidth

### Honest limitation, recorded not buried

X25519 is **not** post-quantum while the signatures are. That asymmetry is
inherited from River and documented on `seal_message` itself, along with
the note that full forward secrecy needs ratcheting and belongs to the
accepted-conversation contract — this envelope is first contact only.

## Full social loop on mainnet: profile → message → read (2026-07-31)

`examples/first_message.rs`, verbatim:

```
sam's profile published: 9mWhRk2Jdp6Hgmt48WHXzXKpbDiB4aRhY9paoC6ArF7T
sam's inbox published:   FzAyxgNAdvi9phZwNbJW39dr9bApsNud2WTofUZApNre

alex fetched and verified: "sam" — say hi if you like bad films
alex sealed a message to sam's inbox and sent it

sam's inbox: 1 pending
  "the worse the film the better. friday?"
```

Alex held nothing but an address Sam chose to share, and that was
sufficient: fetch the profile, verify it, take the published X25519 key,
seal, send. Sam read it back off the network. Nobody else can decrypt it,
and the plaintext appears nowhere in the published bytes (both asserted in
the example, against real network state).

### Profiles now publish an encryption key — and `sign_profile` fills it in

Reachability lives in the profile, not the tile: an encryption key
travelling with a tile would be one more durable handle for a scraper to
correlate. `sign_profile` sets it automatically, because leaving it to
callers means one forgotten field makes an identity silently unmessageable
— a bug that presents as "nobody ever writes to me". Tested, along with the
fact that swapping the key breaks the profile signature (otherwise anyone
could silently redirect a stranger's mail).

### Two failures worth recording

**A demux design flaw turned a clear error into a 120-second silence.**
The reader exited on the first node-side error but left every pending
waiter's oneshot *sender* alive in the routes map, so callers blocked for
their whole timeout and reported `Elapsed` — which says nothing about what
the node refused. Now the reader records the cause and **clears the waiter
map on shutdown**, so receivers resolve immediately; `Demux::await_reply`
turns that into the actual reason. The very next run printed the real
error in seconds.

**That real error was a live schema-migration failure**, and a useful one:
adding `encryption_key` to the signed payload made every client signature
invalid against the **already-deployed** contract WASM, which still
verified the old layout. The v2→v1 fallback exists for exactly this — but
the fallback lives in the *new* code while the network was running the
old. Rebuilding and republishing the contract fixed it.

The lesson generalises: **a signed-payload change is a contract
migration**, not a client change. Delta's `legacy_contracts.toml`
discipline is not optional once real users hold state, because they cannot
be asked to re-sign in lockstep with a deploy.

## THE ANDROID APP RUNS ITS OWN NODE (2026-08-01)

`android/` builds a 36 MB debug APK that installs and runs on a Samsung
Galaxy Z Flip 4 (Android 16). From the app's own log:

```
lkng.node: node started, contributing=true
```

and from inside the app sandbox: **16 distinct peer addresses**, `joined
peer` / `connected peers`, ~140 KB of node log in the first minute, node
RSS **31 MB**. The phone is a Freenet peer, run by the app, with no
desktop involved.

`contributing=true` is the duty-cycle logic working: the phone was
charging on un-metered Wi-Fi, so it took the contributing role. On
battery or on mobile data it runs minimal instead. Both conditions are
required — charging alone on a metered hotspot would spend the user's data
allowance carrying other people's traffic, which is not what "pay with
resources" means to anyone who has ever had a data cap.

### Android specifics that took real work

- **The node ships as `libfreenet.so` under `jniLibs/`, not as an asset.**
  Modern Android will not execute a file from writable app storage (W^X);
  `nativeLibraryDir` is one of the few read-only, executable locations, and
  the packaging only extracts files matching `lib*.so`. Verified directly:
  running the extracted binary prints `Freenet version: 0.2.116`.
- **`Process.descendants()` is a Java 9 API Android does not have**, so the
  Gate-2 finding (the node runs as more than one process, and killing the
  parent leaves a child networking) is handled by sweeping on the
  data-dir path, which is unique to this app and cannot touch anything
  else.
- **`allowBackup="false"`** — identity keys must never ride out in a cloud
  backup the user never thought about.
- **Coarse location only.** Fine location is never requested: position
  becomes a ~5 km cell on device, and there is no reason to hold precision
  we would then have to be trusted to discard.
- **The notification carries a one-tap Stop.** A background networking
  process a user cannot switch off would not be acceptable in an app whose
  whole pitch is not taking things from you quietly.

## Blocking and search (2026-08-01)

### Blocking

`Session::block` / `unblock` / `export_blocks` / `import_blocks`, tested to
hold under every filter (`blocked_tiles_stay_hidden_under_every_filter`).

**Blocks are device-local and never published.** A block list on the
network would tell the blocked person they were blocked, and hand everyone
else a social graph of who avoids whom — which for this user base is
genuinely dangerous. The cost is that blocks don't follow you to a new
device unless the exported blob does; the identity backup is the right
carrier for that, not a contract.

### Search, and where each field is allowed to live

Two tiers, split on how public the data is:

**Profile — exact values, shared by choice.** `Demographics` carries
optional age, height, weight, ethnicity, body type, pronouns and
looking-for, plus free-text search over name, bio and tags. Every field is
optional: nobody should have to state their weight to use a dating app,
and a required field is a field people lie in. Ethnicity is free text
rather than an enum, because a fixed list is a political statement that
always excludes someone and serves mixed-heritage people badly.

**Tile — coarse bands only.** A tile is public to anyone subscribing to a
cell, so exact age/height/weight there would hand a scraper a dossier on
everyone in a neighbourhood. Tiles carry a **decade band** (0 = unstated),
which is enough to filter a grid and far less useful to harvest.

Two rules both tiers share:

- **Unstated never matches a criterion.** Someone who declined to give
  their age is not swept into an age-filtered result — a filter that
  silently includes people who said nothing is a lie.
- **All criteria must hold.** No fuzzy "best match" that quietly widens
  what the user asked for.

Filtering runs **client-side over already-public data and sends nothing**,
so nobody learns what you are looking for. In this category, search terms
are among the most sensitive things a person types.

Demographics are covered by the profile signature, so editing someone's
stated age in transit breaks verification.

## River, fully reviewed — bans and DMs (2026-08-01)

Earlier passes covered River's room state, synchronizer (where the seed-PUT
discovery came from), secrets and membership. This pass read `ban.rs`,
`direct_messages.rs` and `dm_body.rs`, which turn out to answer an
architecture question LKNG had already bet on.

### River tried LKNG's inbox architecture and reverted it — for reasons that don't apply to us

PR #234 built a per-(recipient, room) inbox contract for private DMs; PR
#238 reverted it. The stated reason: it was *"redundant with
freenet/mail"*, and River's own need is better served by **in-room DMs**,
with **freenet/mail for cross-context DMs** — "when users want to message
each other without an existing shared-room context".

**LKNG is exactly that cross-context case.** Two strangers in a geohash
cell share no room, so River's in-room approach has nothing to carry the
message. And River explicitly accepts a tradeoff we cannot: in-room DMs
make *"sender/recipient metadata visible to co-members"*, which is fine
among people who already share a room and catastrophic among strangers in
a neighbourhood, where it would publish who is contacting whom.

So LKNG keeps its Mail-shaped inbox contract — which is precisely the tool
River points at for this case.

**Honest residual, now written down:** anyone subscribing to an LKNG inbox
contract can see envelope count and each sender's *epoch* key. That is a
bounded social-graph signal — bounded because the key rotates per epoch and
is not the durable identity — but it is not zero, and
`docs/anonymity-limitations.md` should say so.

### Confirmations of choices already made

- **Recipient-signed, monotonically-versioned purge tombstones**, with the
  recipient as sole signer — the same shape as LKNG's owner-signed
  `ProcessedSet`. Independent arrival at the same design.
- DM signatures bind sender, recipient, room owner, timestamp and
  ciphertext under a domain tag — the same anti-replay discipline LKNG
  applies to presence records, profiles and envelopes.

### Two techniques worth stealing

- **The `0x80` magic byte.** River prefixes structured DM bodies with
  `0x80`, a UTF-8 *continuation* byte that cannot begin a valid UTF-8
  string — so legacy plaintext bodies and new CBOR bodies are
  unambiguously distinguishable with no version field and no migration.
  Worth adopting when LKNG's message body gains structure.
- **Ban validity is conditional on the banner.** A ban is honored only
  while its banner is the owner or a current, signature-validated member,
  and inert bans are swept. That prevents an un-ban DoS, and a single
  delta is bounded so a forged flood cannot make signature verification
  unbounded. LKNG's moderation feeds should adopt the same rule:
  *a moderation action is only as alive as the authority behind it.*

### A trap LKNG is currently safe from, by luck

River notes (#3987) that `MemberId` is a struct and is **rejected as a JSON
object key**, forcing a `Vec` instead of a `HashMap`. LKNG keys maps by
`[u8; 32]` and serializes with CBOR, which permits non-string keys — so
this works today. It would break the moment anything is serialized to
JSON. Recorded so it is a known constraint rather than a future surprise.

## The app loads its UI from Freenet (2026-08-01)

`fdev website publish` put the compiled Dioxus bundle (551 KB compressed,
4 files) on the network as
**`H477C5kQMNhXDS3H7rfDujjf3fVUghTNm7VHiyFh5ewn`**, and the Android app now
points its WebView at that contract on its own node. From the phone's node
log: `cached contract state`, `subscribed: true`, `failed: 0`.

So the interface is delivered by Freenet rather than baked into the APK.
UI changes ship by publishing a new version — no store review — and the
interface itself has no host to take down.

### Three operational failures, all mine, all worth recording

**1. The node's accept queue was saturated.** `ss` showed `LISTEN 129 128`
— 129 connections queued against a backlog of 128 — so every new client
timed out. The node was healthy; it had simply run out of accept slots
because **every example opened a WebSocket and exited without
disconnecting**. Dozens of runs over 30 hours exhausted it. Fixed at the
source: `Demux::close()` / `FreenetClient::close()` send a `Disconnect`
and every example now calls one. A client that does not hand its slot back
is a slow denial-of-service against its own node, and on a phone that is a
user-visible failure rather than a lab curiosity.

**2. The node was self-updating out from under us.** It exited with code
42 — *"Update 0.2.117 detected, exiting for a service supervisor to apply
it"* — which reads as a crash if you are not looking for it. Updated
`freenet` and `fdev` to 0.2.117.

**3. The placeholder was doing exactly what a placeholder should.** The
phone showed *"provided string contained invalid character '_' at byte
7"*: byte 7 of `REPLACE_WITH_PUBLISHED_UI_CONTRACT_ID` is `_`, which is
not valid base58. A loud rejection beats a silent blank screen.

### Version skew to watch

The APK still bundles a cross-compiled **0.2.116** node while the desktop
runs 0.2.117. Fine for now — they interoperate — but the Android build
needs re-cross-compiling per release, and there is no upstream Android
release asset to pull, so this is a standing maintenance cost rather than
a one-off.

## Styles inlined, so the UI survives being served from a contract

The app rendered as unstyled buttons on the phone while looking correct in
a desktop browser. Cause: Dioxus injects `<link href="/assets/…">` at
runtime, which is **root-absolute**. Under `dx serve` the app lives at `/`
so that resolves; on a node it lives at `/v1/contract/web/<id>/`, where a
root-absolute path points outside the contract and 404s. The WASM loaded
(so the app ran) but the stylesheet never arrived.

Fixed by compiling the stylesheet into the binary with `include_str!` and
rendering it as an inline `<style>`. Path resolution leaves the picture
entirely, and it costs one small file's worth of bytes. Verified by
grepping the *published* wasm for `grid-template-columns`.

This generalises: **anything the UI loads by absolute path breaks once the
UI is served from a contract**, and the failure is partial and quiet —
the app works, it just looks wrong. Prefer embedding over fetching for
anything small.

### A note on verifying on-device

Screenshotting the phone to check the UI captured the notification shade
and personal content instead. Deleted from device and disk, and not
repeated: on-device checks now inspect the DOM or the published artefact
rather than photographing someone's screen. A debugging convenience is not
worth reading a person's messages.

## Atlas, reviewed against our search (2026-08-01)

Atlas's index contract states its division of labour in the first
paragraph: *"Ranking, search, and display are client-side; this contract
enforces only signatures, authorization, structure, bounds, and the
versioned merge."*

That is exactly the split LKNG arrived at — our filtering runs entirely
client-side over already-public data and sends nothing, while the contract
verifies and bounds. Independent agreement from the ecosystem's dedicated
discovery project is about as good a signal as this design gets.

Three things worth taking:

- **Subject ids are random, not derived.** Atlas uses ~72 bits of
  randomness *"deliberately not derived from any attribute, so it survives
  WASM upgrades, URL changes, and owner re-keying."* LKNG derives profile
  addresses from the owner key, which is what makes address-squatting
  impossible — a deliberate opposite trade. But it means **an LKNG user
  cannot rotate their durable key without losing their profile address**.
  That is a real limitation, and if key rotation is ever wanted, an
  Atlas-style random id with a signed pointer is the shape to adopt.
- **Path-traversal checking is done properly and is worth copying
  verbatim** if LKNG ever renders a user-supplied locator. Atlas's comment
  is blunt about why: a hand-rolled check against `..` and `%2e%2e` *"is
  worse than nothing, because it reads as complete coverage while `..%2f`,
  `%2e%2e%2f`, `%252e%252e`, `.%2e` and `..\` all walk straight
  through it"*, and it validates the **whole suffix** — path, query and
  fragment — because browsers normalise dot segments across the entire URL.
- **Printable-ASCII, length-bounded text for anything reaching both a DOM
  text node and a JSON string.** LKNG's headline is user-supplied and
  lands in the DOM; the same discipline should apply before any of it is
  rendered.

Not adopting Atlas as a dependency: it is an explicit work-in-progress RFC
whose own proposal says everything in it is subject to change. Shaping our
moderation actions as signed claims about subjects keeps the door open
without taking the risk.

## Reference review: Open-Grindr, screen by screen (2026-08-01)

Six screens from a Grindr clone that drives the real Grindr API. Useful
because it shows what users of this category actually expect, which is a
different question from what is safe to build.

| Screen | Adopt? |
| --- | --- |
| **1 · Browse grid** with filter chips across the top | Yes — 3 columns matches ours; add the chip row |
| **2 · 1:1 messaging** with expiring images | Mostly — see the honesty note below |
| **3 · Views / Taps tabs** | **Taps yes, Views no** |
| **4 · Advanced filters** | Yes — this drove real gaps, below |
| **5 · Profile page** | Yes, minus the distance readout |
| **6 · Map location picker** | Yes — the plan already requires manual location |

### Gaps it exposed in our model

- **Gender was simply missing.** Their filter offers Men / Women /
  Non-binary / Trans Men / Trans Women / Not specified. For this app that
  is not optional, and it is now `Gender` with named variants plus a
  `SelfDescribed(String)` escape hatch — a closed list always fails
  somebody, and failing somebody's gender here is not a cosmetic bug.
- **"Not specified" is something people search *for*, not only a gap.**
  Our rule that unstated never matches a criterion is right, but it made
  anyone who declined to state a position invisible to every filtered
  search. `include_unspecified_position` makes that an explicit,
  deliberate choice — the two behaviours are different and both are
  needed.
- Their position vocabulary matches ours exactly, with directional arrows
  (↑ Top, ↗ Vers Top, ↕ Versatile, ↘ Vers Bottom, ↓ Bottom, ⇄ Side) worth
  adopting in the UI.

### What we deliberately will not copy

**Distance, everywhere.** Every screen shows it — "11 m", "5.6 km", even
in the chat header. That is exactly the mechanism behind the documented
trilateration attacks on this category. LKNG shows cell membership and
nothing finer, and `tiles_carry_no_distance` fails the build if a distance
field ever appears in the render model. This is our largest UX divergence
and it is the entire point of the project.

**"Views" — who looked at your profile.** Implementing it requires logging
every view, which is precisely the surveillance apparatus this project
exists to refuse. **Taps are fine** and will be built: a tap is an explicit
act by the person doing it, not a record kept about someone who did
nothing.

**"Expiring images" need honest wording.** Media in LKNG is replicated
ciphertext; revocation is prospective only. A timer can hide an image in
our client and stop distributing it, but cannot reach copies already
fetched. If this ships, it says "hidden after 10 minutes", never
"deleted" — anything stronger is a promise the network cannot keep.

## Favourites and private notes (2026-08-01)

Both are in `AddressBook`, and both are **device-local, permanently**.
Favourites are an interest graph; notes are by definition things you wrote
about someone who did not agree to it; blocks reveal who you avoid.
Publishing any of the three — even encrypted, even to your own contract —
would put a standing record of your attractions, judgements and avoidances
on other people's disks forever. They leave the device only inside the
passphrase-encrypted identity backup.

### The durable-handle problem

Tiles carry per-epoch pseudonyms that rotate, which is what stops a scraper
building a movement history. That same rotation means a favourite or note
attached to a pseudonym would evaporate at the next epoch: **the privacy
property and the feature are in direct tension.**

Resolved by following what you actually know about a person:

- **After a match** you hold their durable profile key, so favourites and
  notes attach to that and last. This is the case that matters — notes are
  for people you have dealt with.
- **Before a match** you know only a rotating pseudonym, so a pin is
  session-scoped. `pin_for_session` returns `false` specifically so the
  caller cannot ignore that and must tell the user.

The alternatives were both worse: leak a durable handle onto tiles
(defeating rotation) or silently lose the user's notes at an epoch
boundary.

`nothing_in_the_book_is_publishable_state` exists as a guard — if someone
later adds a path from the address book to the network, that test is where
they have to argue for it.

### Screens still to build

From the Grindr/Open-Grindr surface: albums and videos, taps, messages
list, settings, profile editing, sign-up and login. Sign-up and login are
*not* account screens here — there is no account. They are key generation
into the Keystore and passphrase recovery, which already exist and tested
in `lkng-identity`; what is missing is the UI.

## Official skills caught a real design error (2026-08-01)

`freenet/freenet-agent-skills` (LGPL, official, updated daily) documents
the patterns this project spent the session reverse-engineering from
source. Its `identity-and-addressing.md` **corrected a decision we had
already shipped**.

We put the full 1952-byte ML-DSA-65 verifying key directly in
`ProfileParams` and `InboxParams`. The guidance is explicit: *"whatever key
material you use, keep it out of identifiers and out of routinely
transmitted parameters."* Parameters travel with **every** GET, PUT and
subscribe, so ours were ~1979 bytes each. Delta had been showing the right
shape all along (a short base58 prefix in parameters) and we did not
follow it.

Now: `params = { schema_v, address: [u8; 16] }` where
`address = BLAKE3(signing_key)[..16]`, the full key lives in **state**, and
`verify_state` rejects any state whose key does not hash to the address —
so the identifier stays self-certifying: given an address you can fetch the
contract, read the key, and check it yourself with no directory and no
trusted lookup. `parameters_carry_no_key_material` fails the build if key
bytes ever reappear there.

**16 bytes is a security parameter, not a UX one.** The property that
matters is second-preimage resistance: if an attacker can grind a
*different* keypair hashing to your address, they can serve state at your
address carrying their key and the binding check passes for both. No
contract-side tie-break saves this — "first writer wins" is unenforceable
in a permissionless eventually-consistent store, and any deterministic
ordering on key bytes is itself grindable. Length is the only defence, so
handles went from ~12 to ~22 characters on purpose.

**And it solved the rotation problem Atlas raised.** Derive the address
from the **signing key only**, keep the encryption key as ordinary signed
state — then a user can rotate their encryption key without their address,
or anyone's link to them, changing.

## Account transfer to a new phone

Three paths, deliberately distinct:

1. **Passphrase recovery over Freenet — no file, no cloud.** The identity
   is a 32-byte seed; `backup_locator(passphrase)` addresses a contract
   derived from the passphrase alone. Install on a new phone, type the
   passphrase, fetch, decrypt. The network is the backup.
2. **An encrypted file the user can put anywhere**, Google Drive included.
   `to_backup_with` now seals the seed **and an opaque application blob**
   (favourites, notes, blocks) in one file. The blob is opaque on purpose:
   the crate holding key material must not grow a dependency on
   application types to move somebody's notes.
3. **Automatic cloud backup: deliberately not enabled.** `allowBackup` is
   `false`, so Android will never sweep identity keys into a cloud backup
   the user never thought about. An explicit, user-initiated export to
   Drive is a different act from a silent one, and only the first is
   theirs to consent to.

**Chat history comes back for free.** Messages live in the inbox contract
on the network, encrypted to the identity — so restoring the seed restores
the ability to read whatever is still there. No separate chat backup, and
nothing to opt into.

The honest caveat, already in the code: a weak passphrase is
brute-forceable offline by anyone who guesses the derivation, which is why
Argon2id runs at 64 MiB / 3 passes and the UI must enforce a strength
meter.

## Upgrade discipline, checked against official guidance (2026-08-01)

`upgrade-and-migration.md` opens with a design precondition that decides
whether upgrades are routine or catastrophic:

> Choose a stable identity anchor that is independent of the WASM; never
> expose the raw content-addressed contract key as your app's identity.

**We satisfy this**, and only just — because of the address change made
hours earlier. A contract key is `BLAKE3(BLAKE3(wasm) || params)`, so it
moves on *any* WASM change: a dependency bump, a compiler change, a
one-line fix. LKNG's durable identity is
`address = BLAKE3(signing_key)[..16]`, which involves no WASM at all, so
clients re-derive the new contract key and every reference still points at
the right person. River anchors the same way (invites embed the room
owner's verifying key), and its live 0.6→0.8 stdlib re-key migrated every
room with no recreation and no broken invites.

Had we kept shipping raw contract keys as identity — or the full key in
parameters — every profile link, every match, and every inbox reference
would break on the next `freenet-stdlib` bump. That is not a bug you fix
after launch.

**The gap we do have:** no carry-forward. When a contract's WASM changes,
its old state is currently orphaned — we saw this today when adding
`encryption_key` produced a new contract with none of the old state. The
anchor is right, so references survive; the *state* does not follow yet.
Delta's `legacy_contracts.toml` is the mechanism, and it needs adopting
before anyone but us holds data worth keeping.

Guidance worth repeating verbatim: *"Read it before your second release,
and design for it before your first."*

## Keystore-sealed identity (2026-08-01) — built, not yet confirmed on device

`KeyVault.kt` seals the identity seed with a non-exportable AES-GCM key in
the Android Keystore and exposes exactly three methods to the WebView
(`get`, `put`, `isSealed`) — no paths, no URLs, nothing that becomes a
general capability. The web side prefers the vault and falls back to
`localStorage` only where the bridge is absent (desktop development).

**What it does and does not protect.** Copying app storage now yields
ciphertext that opens nowhere else, and the wrapping key cannot be
extracted even by this app. Script running inside our own WebView can
still ask the vault to unseal, because the seed has to reach the WASM
crypto — closing that means moving all signing behind the bridge so the
seed never crosses it. That is the right end state and a much bigger
change. Stated in the class doc rather than implied.

Two deliberate refusals:

- `setUserAuthenticationRequired(false)` — the node must sign presence
  while the screen is off, and a key needing a fingerprint every epoch
  would make the app silently stop working in a pocket.
- The method is `isSealed()`, not `isHardwareBacked()`. `KeyInfo`
  introspection varies by API level and vendor, and claiming hardware
  backing on a device where it is absent would be worse than claiming
  nothing. The UI says "sealed by this device's keystore", which is
  checkable.

### A migration bug worth remembering

The first version had `vault_get` fall back to `localStorage`, which
**succeeded** for any install that already had a seed — so `put` was never
called and the vault stayed empty. The security fix would have applied
only to people who installed after it, leaving existing users (whose keys
had been exposed longest) on web storage forever. Now a fallback read
migrates: seal into the vault, and only remove the plaintext copy once the
sealed one is confirmed stored, because losing the seed is losing the
account.

### Also fixed: the WebView was caching the self-updating UI

The UI ships over Freenet at a URL that never changes while its content
does, so HTTP caching pinned the app to whatever version it first loaded —
**including past a security fix**. `LOAD_NO_CACHE` plus `clearCache` on
start; the content comes from loopback, so caching bought nothing anyway.

### Status: unconfirmed on device

After all of the above, `shared_prefs/lkng.vault.xml` is still not being
created on the test phone, so the seal has **not** been observed working
end to end. The node is healthy and the app runs, so the likely causes are
the published UI not yet having reached that phone's node, or the bridge
not being reachable from inside the node's sandbox iframe. Until this is
confirmed, **the release blocker stands** — the code is right but unproven,
and an unproven security control is not one.
