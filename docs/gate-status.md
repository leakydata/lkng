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

### Confirmed on device — and the real cause was neither suspect

`shared_prefs/lkng.vault.xml` now holds `lkng.identity.seed.v1` and
`lkng.jitter.secret.v1`, written by the WASM through the bridge from
**inside** the node's sandbox iframe. The seal works end to end.

Both of my hypotheses were wrong, and finding that out took isolating one
variable at a time:

1. A **Kotlin-only self-test** (seal, unseal, compare — no JavaScript)
   logged `stored=true roundtrip=true sealed=true`. So `KeyVault` was
   never the problem.
2. A **frame probe** reporting what the page could see returned
   `chrome-error://chromewebdata/`. **The page had never loaded at all.**

The actual bug was a **startup race**: `MainActivity` started the node
service and called `loadUrl` in the same breath, but the node needs tens of
seconds to bind its port. Every cold start hit connection-refused. It
appeared to work only when a node happened to survive from a previous run
— precisely the case a developer sees constantly and a new user never
sees. Left in, **the app would have failed on first launch for everyone.**

Fixed by polling loopback until the node answers, showing honest progress
("Starting your node and joining the network… first run takes about a
minute") rather than a spinner, and giving up after ~2 minutes with a
message that admits what happened. Polling rather than a fixed delay
because a cold, slow or busy phone beats any constant you would pick.

The debug frame probe is gone; the Keystore self-test stays as a startup
assertion, since it turns a silent security regression — keys quietly
falling back to web storage — into a visible one.

## Real location (2026-08-01)

`Locator.kt` exposes one thing to the UI: a coarse position. The web layer
no longer holds precise coordinates at any point.

**Why not the WebView's own Geolocation API.** It would hand the page raw
lat/lon at whatever precision the OS offers. The web layer has no use for
that — it converts position to a ~5 km cell and discards the rest — so the
precision would exist purely as something that could leak, through a bug,
an XSS, or a future careless feature. Asking natively for **coarse only**
means the grid *cannot* render a distance, because nothing in the app ever
knows one.

Three deliberate narrowings:

- **`ACCESS_COARSE_LOCATION` only, never `FINE`.** Android fuzzes coarse
  location to roughly a 1–2 km grid before the app sees it, which composes
  with our own stable jitter instead of replacing it.
- **No background location.** Presence is published while the app is open.
  An app in this category asking to track you when closed would deserve
  the suspicion it got.
- **Last-known fix, not a live one.** A fresh fix costs battery and buys
  precision that is immediately quantised away; a position from minutes
  ago is indistinguishable after a 5 km cell.

**The fallback is labelled, not disguised.** When there is no position —
permission refused, or a desktop build with no bridge — the UI shows a
sample area with a `sample` badge on the cell and says so in the status
line. Showing someone a grid of people 3,000 km away as though they were
nearby would be worse than showing nothing.

## Profile editor and photos (2026-08-01)

A Profile tab with name, headline, age, gender, position chips, HIV-status
chips, bio and a photo. Everything is optional **except age** — a required
field in an app like this is a field people lie in, and a lie in a profile
is worse than a blank. Age is the exception because 18+ is a legal
obligation everywhere this could ship.

### EXIF is stripped by construction, not by parsing

The chosen photo is drawn to a `<canvas>` and re-encoded. A canvas holds
**pixels, not a file container**, so orientation tags, camera model,
timestamps and — the one that matters — **GPS coordinates** do not survive
the round trip. Nothing in `photo.rs` parses EXIF, which means nothing in
it can miss a variant of EXIF.

This is not a nicety. A phone photo routinely carries the exact coordinates
it was taken at. Publishing one to a public presence tile would hand any
scraper the precise location that the entire cell-and-jitter design exists
to withhold — **a complete bypass of the app's central privacy property,
through its most ordinary feature.** A parser could be out of date; a
canvas cannot be.

The UI says so where the user is choosing: *"Your photo is resized on this
device and its location data is removed before it is ever published."*

### Size is enforced before anything is signed

256 px square, centre-cropped so faces are not distorted, re-encoded to
WebP with a descending quality ladder until it fits under 16 KiB — and
**failing rather than publishing something oversized**. Every phone in a
cell downloads every tile in it, so thumbnail bytes are the single number
deciding whether the grid is usable on mobile data.

### Health data is labelled where it is entered

The status chips carry: *"Health information stays in your profile and is
never put on the public grid. Only people you share your profile with can
see it."* The enforcement is a test; the sentence is so the user knows it
without reading one.

**Not yet wired:** saving writes the draft to this device only. Publishing
the signed profile and republishing the presence tile with the new
thumbnail is the next step — the crypto and contracts for both already
exist and are tested.

## 2026-08-01 — the stale-bundle day

**Symptom:** the phone kept showing the old UI after every republish.

**What I concluded, repeatedly and wrongly:** the phone's node had cached an
old contract version; Freenet's self-update path was broken; a GET returns
local state without checking for a newer version. I force-refetched the
contract over `adb forward`, published directly into the phone's node, and
told the user the self-update path "doesn't actually work yet".

**Actual cause:** `dx bundle --platform web --release` re-emitted a previous
build's wasm while reporting success. Every publish that day shipped
identical, months-stale bytes. Deleting `web/target/dx/lkng-web/release`
changed the content hash immediately (`dxh39b9bdecb45f69cc` →
`dxhfa24f2e1b4517085`).

**How it was finally caught:** stamping a unique string into the source and
grepping the *compiled wasm* for it. Every earlier check compared the phone
against the desktop — and they matched perfectly, because both were serving
the same stale build. Comparing two copies of the same wrong thing looks
exactly like success.

**Two lessons worth more than the fix:**

1. A build step that can silently emit stale output makes every downstream
   diagnosis fiction. The reasoning about caching and eviction that day was
   sound; the premise was false, so the conclusions were confident and wrong.
2. Verify against *ground truth*, not against another derived copy. "Phone
   matches desktop" was checked several times and was never evidence.

**Fix:** [`scripts/publish-ui.sh`](../scripts/publish-ui.sh) deletes the
bundle dir, stamps a marker, and **refuses to publish** if the marker is
absent from the wasm. The marker renders in the app footer, so "is this
device running the new build?" is answerable by looking at the screen —
no adb, which is also how a real user would have to answer it.

**Correction to the record:** the earlier gate-status claim that Freenet
contract self-update is broken is withdrawn. It was never tested — the same
bytes were being published each time, so of course nothing changed.

## 2026-08-01 — messaging, and two more build-ordering traps

**Tab bar restored.** Removing it when the avatar menu landed was a
misreading of the request: Grindr has both, and they answer different
questions. The tab bar switches between *what you do* (browse, taps,
messages, albums); the avatar menu holds *who you are* (profile, settings).
Merging them made the frequent action slower and the rare one easier to hit
by accident.

**Per-epoch encryption key on presence records.** A stranger has to be able
to encrypt a first message, and there is no handshake to piggyback on, so
the key must be public — a tile is exactly that. It is derived from the
**epoch** identity, not the durable one. A durable key here would have
silently undone pseudonym rotation entirely: a scraper joining every cell
could record it and link one person across every epoch and area they ever
appeared in, a permanent identifier wearing the costume of a rotating one.
Two guard tests hold the line: `encryption_key_rotates_with_the_epoch` and
`a_swapped_encryption_key_breaks_the_tile_signature` (the second because an
unsigned key would let anyone who can write to a cell substitute their own
and read every "encrypted" reply).

**Sample profiles cannot be messaged.** Demo tiles are built through the
real signing path, so they now carry real encryption keys — derived from
constant seeds compiled into the binary and therefore known to anyone with
the source. Sealing to one would look like encryption and be publicly
readable, which is worse than refusing, because the user would believe it
was private. The Message button is disabled with the reason stated.

**Two ordering bugs, same shape as the stale bundle:**

1. `sed` replacement text containing `&str` -- `&` means "the whole match"
   in a sed replacement, so the marker line was spliced into itself. It was
   caught only because the corruption happened to break the parse. Had the
   marker lived in a string that still compiled, the verification script
   would have gone on certifying builds while corrupting the file it checked.
   Now done in python.
2. The web bundle embeds contracts via `include_bytes!`, and a contract key
   is `BLAKE3(BLAKE3(wasm) || params)`. Building the UI before the contracts
   ships an app that addresses contracts which do not exist -- it looks
   completely healthy and can talk to nobody. `publish-ui.sh` now builds
   contracts first, unconditionally.

All three of today's bugs were the same bug: **a build step that silently
produced something other than what the source said.** None was a logic
error, and none would have been found by reading the code.

## 2026-08-01 — tile → message proven on mainnet, and why the inbox rotates

`tile_to_message` now runs green against the live network: Sam posts a tile,
Alex reads the cell exactly as a scraper would, seals a message using
**only** what the tile carried, and Sam reads it. No profile is exchanged
and no handshake happens.

**The bug it caught is the interesting part.** The first run failed on the
network with `inbox failed verification: signature verification failed`.
Cause: the inbox was addressed by the recipient's *durable* verifying key,
but a stranger holding a tile can only know the recipient's *epoch* key —
that is the whole point of signing tiles with subkeys. So the envelope was
bound to one key while the contract was addressed by another.

This was not a coding slip; it was a design hole with no local symptom. Every
unit test passed, because in a test both parties are the same process and
"the durable key" is simply available. It could only fail where it did: on
the wire, between two parties who genuinely know different things.

**Fix:** inboxes are addressed by the epoch key and the client watches the
**current and previous** epoch — the same construction the grid already uses
so a rollover never empties it. Stated cost: mail older than two epochs
(12 h) is not collected. Epoch keys are derived from the master seed rather
than discarded, so nothing is cryptographically lost; today the client simply
does not look further back.

The example also asserts, against the bytes that crossed the wire, that
neither the durable verifying key nor the durable encryption key appears in
cell state. That assertion is the one guarding the property that would
otherwise fail *silently*: if someone later "simplified" the tile's
encryption key to a durable one, messaging would keep working perfectly and
every user would quietly become trackable across every epoch and area.

## 2026-08-01 — the app was never in its own grid

Found while looking for something else: `compose_tile` appeared **only** in
`demo_tiles`. The app subscribed to cells, verified tiles, rendered a grid,
and never published the user's own. Every install was a spectator — invisible
to everyone, and unmessageable, because a tile is what advertises the
encryption key a stranger needs.

This is worth recording because of *how* it hid. Every visible surface
worked: the grid filled, tiles verified, the network round-tripped. All the
evidence pointed at a working app, because everything that produced evidence
was the read path. The write path had no symptom at all — an app that
publishes nothing looks exactly like an app in an empty neighbourhood.

**Publishing is gated on three conditions**, and the middle one is the one
that would have been easy to get wrong:

1. the user has written a headline — otherwise the app puts someone in a
   public grid before they have decided to be there;
2. there is a **real device fix**. Publishing against the sample location
   would drop the user into a cell on the other side of the world, visible
   to strangers, having never left their house. In development the sample
   area looks like it works, which is exactly why this needs to be a
   condition rather than a habit;
3. the node is connected.

Re-published every four minutes, because cells cap at the newest N records
(a stale tile is evicted by new arrivals) and cells are per-epoch contracts
(at a rollover the tile must be written into a *different* contract or the
user silently vanishes). Four minutes and not four seconds: every publish is
bytes that every phone in the cell downloads.

The UI now states plainly whether you are visible and, if not, why. The
condition is computed once and shared with the publisher, so the app cannot
claim a visibility it has not acted on.

## 2026-08-01 — presence publishing was broken the moment it was written

`two_apps` was written to exercise the app's actual write path —
**seed-then-update** — because `two_strangers` and `tile_to_message` both
prove discovery via a single PUT of a full cell state, which is not what the
app does. It found two bugs on its first two runs.

**1. The delta was the wrong shape.** `publish_presence` encoded a
`CellState` (a CBOR map). The contract decodes `Vec<PresenceRecord>` and
rejects anything else:

```text
delta: Semantic(None, "invalid type: map, expected array")
```

So the tile never landed. Nothing in the app reported this: the update was
dispatched, the UI said "You're visible", and the write failed inside the
contract. `Session::tile_delta` — the correct encoder — already existed in
`lkng-app` and was simply not used. The feature was broken from the moment
it was written, a few hours earlier, in the same session that had just
finished writing up why unverified write paths are dangerous.

**2. An unconditional seed on a republish timer.** `publish_presence` PUT a
full contract container every four minutes, forever, for a contract that
already existed. In the examples' demux a timed-out PUT kills the whole
session; in the browser client it is only recorded as `last_error`, so the
app survived it — and would have gone on quietly pushing a contract
container into the network on a timer, paid for by every peer nearby, with
no symptom at all. Now `Node::seed_once`.

**The lesson, which is the same one as this morning's stale bundle:** both
bugs were in code that had been read carefully, reviewed against the
contract, and reasoned about correctly at every step except the one that
mattered. Neither was visible from the source. Both took about ninety
seconds to find once something actually ran the path end to end.

Reading is not verification. An example that exercises the real path is.

## 2026-08-01 — the Android build had not compiled Kotlin in some time

Setting up release signing surfaced this: Kotlin 2.0.21's compiler parses
the JVM version string with a routine that rejects a bare major version of
25, throwing `IllegalArgumentException: 25.0.3` before compiling anything.
The machine's default JDK is 25.

**The part worth recording is how it stayed hidden.** Because every output
was already up to date, `gradle assembleDebug` reported BUILD SUCCESSFUL and
produced an APK — the previous one, unchanged. Earlier tonight that APK was
built, installed on the device, and reported as "the latest build". It was
not; it was whatever had last compiled successfully under a JDK that worked.

`org.gradle.java.home` now pins JDK 21, and a clean build confirms
`compileDebugKotlin` actually runs rather than being skipped.

**Three times in one day**, on three unrelated toolchains:

| Tool | Reported | Actually did |
| --- | --- | --- |
| `dx bundle --release` | success | re-emitted a previous wasm |
| `gradle assembleDebug` | success | skipped Kotlin, kept the old APK |
| `publish_presence` | "You're visible" | sent a delta the contract rejected |

Not one was a logic error, and not one was visible by reading the code.
Each was found only by checking the *artifact* against something independent
— a marker string in the wasm, a task list in the build log, a record fetched
back off the network.

The rule this session earned: **a build or a write that reports success has
told you about itself, not about its output.** Verify the output.

## Release signing

`android/app/build.gradle.kts` reads `android/keystore.properties`, which is
git-ignored and does not exist in this repository. No release key has been
generated — that key is the entire update-trust model for an app in this
category, since whoever holds it can ship an update every existing install
accepts silently. It should be created and held by the project owner, on a
machine of their choosing, and backed up offline; Android app signing keys
cannot be rotated for an existing listing.

Release builds without it are produced **unsigned** rather than falling back
to the debug key. A debug-signed release APK installs and looks fine, which
is exactly how one ends up distributed.

## 2026-08-01 — reporting, shaped as Atlas descriptors

`lkng-moderation` plus `contracts/moderation`. A report is a signed claim by
one party about a subject; a feed is a contract that accumulates them; a
client subscribes to the feeds it trusts. There is no authority, because
there cannot be one — no server to enforce a ban, no account to suspend, no
way to make a peer drop data it wants to keep. Building a "remove this
person" button would have been building a feature that silently does
nothing.

Design points that are load-bearing rather than incidental:

- **Reports are counted by reporter, not by report.** `reporter_count`
  deduplicates on the signing key. Counting reports is how one determined
  person manufactures the appearance of consensus against someone.
- **The feed name is inside the signed payload.** Without it, reports could
  be harvested from a permissive feed and replayed into a strict one.
- **Signed with the epoch key, never the durable one.** A report carries its
  verifying key in public; the durable key would tie every report a person
  ever files to one permanent identity and, through it, to their profile
  address. `Identity::sign_report` takes `&self` deliberately so the caller
  must pass `for_epoch(e)` — there is no correct epoch to derive, since feed
  parameters carry none, and guessing would be worse than asking.
- **Cap applied post-merge, never in validation**, and truncation ordered
  totally by `(timestamp, id)` — the same discipline as the presence cell,
  for the same convergence reason.
- **The contract verifies reports on the way in** and drops bad ones rather
  than failing the update. One unverifiable record reaching state would make
  the next `validate_state` reject the whole feed, turning a single bad
  report into a feed nobody can write to.

**What the UI says, and why it says it.** The report sheet states that there
is no company to appeal to, that a report removes nobody, and that blocking
is the only thing that actually stops someone reaching you. After filing, it
says the report is *pseudonymous, not anonymous*: the epoch key that signed
it is the same one on the reporter's tile, so someone watching both can tell
which tile filed it. That matters most in exactly the case reporting matters
most — a small cell, a dangerous person. Harvest's blind-signed tokens are
the real fix and are not built; until they are, the honest move is to say so
where the user can read it, not only in a doc.

**Proven on mainnet** (`report_flow`): five reports from three people land
in a feed, all verify against fetched bytes, and `reporter_count` returns
**3, not 5**. No reporter's durable key appears in 27 339 bytes of feed
state. Running it twice produces byte-identical state, which also
demonstrates content-id dedup — a replayed report is not a second report.

Written before trusting the path, on the principle the day established: two
write paths tonight were broken in ways reading them could not reveal.

## 2026-08-01 — photos were captured, published, and never shown

Third instance of the same shape as the presence bug, found the same way —
by asking what a feature actually does end to end rather than whether its
pieces exist.

Photos were: chosen, canvas-re-encoded (so EXIF and GPS cannot survive),
size-laddered under 16 KiB, signed into the presence record, and published
to the cell. Every one of those worked. `Tile::thumbnail` then reached the
UI and **nothing rendered it** — `TileCard` drew `swatch(pseudonym)`, a
deterministic gradient written as a placeholder "before photo support
lands". Photo support had landed; the placeholder stayed.

So the whole grid was gradients, and a user who added a photo saw it only on
their own avatar. The bytes were on the network the entire time.

Now `tile_art` renders the published thumbnail as a `data:` URL, falling
back to the swatch when there is none. The fallback is deliberate rather
than lazy: a grid of empty boxes reads as broken, and a grid where
photo-less people are invisible quietly punishes everyone on their first
run. `peer_art` does the same for conversations, falling back when a tile
has expired — losing the picture after six hours is expected, losing the
thread would be a bug.

A `data:` URL also means rendering the grid makes no third-party request,
so a supplied photo cannot be turned into a beacon that reports who viewed
whom.

## 2026-08-01 — contract migration, and a doc comment the network refuted

`contracts/legacy_contracts.toml` plus `scripts/snapshot-contract.sh`: a
contract's address is `BLAKE3(BLAKE3(wasm) || params)`, so any change to
compiled output — a struct field, a dependency bump — moves it. The old
address keeps everyone's data, the new one starts empty, and nothing errors.
The snapshot script copies the current wasm before such a change, because
afterwards the old binary does not exist. The blobs are committed on purpose:
a migration path requiring a months-old toolchain rebuilt to byte-identical
output is not a migration path.

Tonight's presence-cell rotation is recorded there as unrecoverable. It cost
nothing real — presence cells are epoch-scoped and everything in them was
hours from expiring — but the same oversight on `inbox` would have been
every message anybody had received.

**The part worth recording.** I wrote `carry_forward` with a doc comment
asserting that envelopes are bound to the recipient's key rather than the
contract address, so migrated state would republish cleanly. Three unit
tests passed. The mainnet run rejected it in one round trip:

```text
UPDATE failed: inbox failed verification: signature verification failed
```

An envelope's signature covers the **inbox parameters**. Mail sealed to a
retired address does not verify at a new one — and that is the binding which
stops anyone lifting an envelope out of one person's inbox and replaying it
into another's. It is a property to keep, not an obstacle.

So migration is a **read**, not a move: fetch the retired address, decrypt
what you can, keep the plaintext on the device — which is already where the
user's sent messages live and already authoritative for history. The network
delivers; the device archives. `carry_forward` is now `#[must_use]` with the
warning in its own docs, and the property is pinned by a test in
`lkng-identity` so nobody re-derives the wrong conclusion from the types.

No unit test could have caught the original error: in-process, nothing checks
the parameters. It needed a contract that does.

This is now the fourth time today that something confidently correct on
paper was wrong in fact, and the fourth time the difference cost one round
trip to find.

## 2026-08-01 — albums: ciphertext on the network, keys per person

`lkng-album` + `contracts/album`. Everything here follows from one fact: **a
private photo on a public network cannot be un-shared.** Freenet replicates;
there is no server to delete from and no way to make a peer forget bytes it
holds. So the design cannot rest on withdrawing access later — the bytes
must never have been readable.

The album contract therefore holds **ciphertext only**. Anyone may fetch it
and it is meaningless to them. What is shared is the key, sealed
individually to each named person and delivered through the inbox, where a
grant is indistinguishable on the wire from an ordinary message: nobody
replicating an inbox can tell that an album was shared, with whom, or that
one exists.

Splitting key from photos also means adding a viewer costs one small message
rather than a re-upload — an album shared with ten people is uploaded once.

**Revocation is prospective, and that is in the data, not just the prose.**
Photos carry a key `generation`; removing someone bumps it. `readable_at`
returns everything at or below a viewer's generation and nothing above. Two
tests hold both halves:

- `a_removed_viewer_sees_nothing_added_afterwards` — the guarantee;
- `removal_does_not_take_back_what_was_already_shared` — the honest converse,
  asserted so nobody later "fixes" it into pretending otherwise. They have
  the ciphertext and the old key; those are facts about their device now. A
  data structure cannot take them back and the UI must not imply it can.

Other decisions worth their weight:

- **There is no plaintext photo type.** A struct that *could* hold a
  cleartext photo is one somebody eventually serialises into a contract by
  mistake, and on this network that mistake is permanent.
- **An album verifies only at an address derivable from its owner's key.**
  Without that check anyone could sign their own album, place it at
  someone else's address, and a grant would point viewers at an impostor.
- **Generation never moves backwards** in the contract. A replayed older
  state would reinstate a key a removed viewer still holds — silently
  undoing a revocation the owner believes happened.
- **Grants carry a domain prefix**, so a text message can never decode as a
  grant and a grant can never be rendered as text somebody typed.
- **The durable key signs an album**, unlike a tile. Acceptable here
  precisely because an album's address is known only to grantees, so the key
  is not sitting in a public cell for scrapers.

161 tests. UI still to come; the crate and contract are done and tested.

**Proven on mainnet** (`album_share`): an album is published, fetched by
anyone (5 665 bytes, verified), and none of the plaintext appears in those
bytes. A stranger holding the whole album cannot read it. A friend, using
only what arrived in their inbox, reads both photos. After revocation the
album is generation 2: the removed viewer still reads the two old photos and
cannot read the new one.

That last line is the design stated as an observation rather than a promise.
The bytes they already had are theirs, and no contract can take them back.

## 2026-08-01 — receiving album grants, and a near miss

Albums could be shared but not received. The UI now collects grants from the
same inbox states messages come from, fetches each granted album, verifies
it, and renders what the grant's generation permits.

**The near miss.** A grant's payload begins with the ASCII bytes of its
domain tag, not with one of this module's marker bytes — so before the fix
it fell through to the legacy plain-text branch and would have rendered *a
private key* as a wall of mojibake inside a conversation. Recognisable to a
user only as a bug; recognisable to anyone reading over their shoulder as
something else. `decode_payload` now checks for a grant first, and
`encode_payload` deliberately cannot produce one: grants are built by
`lkng-album`, which owns their format, so a caller cannot synthesise a
keyless grant whose failure would land on the recipient.

Two rendering decisions that are about honesty rather than polish:

- **A granted album is verified before anything is drawn.** A grant names an
  address; without checking that the album there is signed by the key named
  in the grant, a grant is an instruction to fetch and display a stranger's
  contract.
- **A photo that will not open is drawn as an empty frame, not skipped.**
  It means the owner rotated the key. A silently shorter grid would hide
  that anything had changed; an empty frame says "there is something here
  you are no longer meant to see", which is the truth.

Also worth recording: the first attempt at this patch silently did nothing.
The anchor text it matched on had been rewritten earlier in the session, so
the replacement applied zero times and the build stayed green. Caught by
grepping for the new identifier rather than trusting "no errors" — the same
class of failure as the stale bundle, at a smaller scale, and the same fix:
check the artifact, not the report.

## 2026-08-01 — account backup and restore

The crypto had existed and been tested since the first week; there was no
screen. So "losing the phone loses the account" was true of every install,
and the README said so while the fix sat unused in a library.

The backup carries the seed **and** the app data that exists nowhere else —
profile draft, sent messages, album key. A backup of only the key would
restore an identity with no history, which is not what a person means by
getting their account back.

Two details worth their weight:

- **Restore writes the seed through the same vault path a fresh install
  uses**, so a restored account is Keystore-sealed like a new one. A restore
  that quietly left the key in plain web storage would downgrade exactly the
  person who had most reason to trust the backup.
- **Wrong passphrase and damaged file give one message.** AEAD failure is
  indistinguishable between the two, and inventing a distinction would send
  someone hunting a problem they do not have.

The passphrase warning talks about *length*, not symbols, because the threat
is offline brute force against a file the user has stored somewhere. Argon2id
at 64 MiB / 3 passes makes each guess expensive; it cannot make a common
passphrase safe, and the copy says so rather than showing a green bar.

Still missing: nothing prompts for a backup during setup. It is a settings
screen a user has to go looking for, which for the one action that prevents
permanent account loss is the wrong shape. Onboarding should offer it.

## 2026-08-01 — the mainnet smoke suite

`scripts/mainnet-smoke.sh` runs all five proofs in sequence against the live
network. Current result: **5 passed, 0 failed** — presence via seed-then-
update, tile-to-message, reporting, album sharing with revocation, and
migration.

Every one of these examples exists because the path it covers shipped broken
after being written carefully and read closely. So the suite is not a demo
set; it is the only evidence the write paths work, and it needed to be one
command rather than five things to remember before a release.

It retries once per example, deliberately: mainnet PUTs time out under peer
churn, and distinguishing that from a real failure is the difference between
a useful signal and one people learn to ignore. Two failures in a row is
reported as a failure.

**What it does not cover, stated in the script's own output** so nobody
mistakes a green run for more than it is: the UI, the WebView, the Android
node lifecycle and battery cost are all outside it. Two people on two phones
remains the test that matters, and has still not happened.

## 2026-08-01 — self-update over Freenet works, and the bundle was twice the size it needed to be

**Two findings, one good and one embarrassing.**

**The self-update path works.** The phone, dozing and locked with only its
node running, was found holding build `b37214` — byte-identical to the
desktop, fetched over Freenet with no adb involved. This morning I concluded
the opposite and told the user so. That conclusion was drawn while
republishing identical stale bytes, so the phone had nothing to fetch and
"it never updates" was indistinguishable from "there is nothing new". The
earlier retraction stands; this is the positive confirmation.

The check that settles it, and the reason the marker exists: grep the
*served wasm* for the build marker. Comparing the phone against the desktop
proves only that two things agree, which they did all morning while both
were wrong.

**The bundle had no size optimisation at all.** The release profile was
stock, and the wasm had grown 3.63 MB → 6.74 MB in one evening as messaging,
albums and moderation landed. Adding `opt-level = "z"`, LTO,
`codegen-units = 1`, `panic = "abort"` and `strip`:

| | wasm |
| --- | --- |
| start of evening, fewer features | 3.63 MB |
| after tonight's features, stock profile | 6.74 MB |
| after tonight's features, size profile | **2.58 MB** |

Smaller than it started, with far more in it.

This matters more here than in most apps. The bundle is fetched over Freenet
by every user, on a phone, and re-fetched on every UI update — carried by
peers who did not ask for it. A project whose central claim is that it does
not treat other people's resources as free had shipped an unoptimised 6.7 MB
binary all evening. `opt-level = "z"` over `"s"` because nothing here is
compute-bound: the heaviest operation is an ML-DSA verification in
milliseconds, and nobody notices that who is waiting on megabytes.

**Propagation, measured.** Build `b37598` was published to the desktop node
at 22:28 and served byte-identical (2 710 288 bytes) by the phone's node
within ~15 minutes — the phone dozing, screen locked, app in the background,
only the foreground-service node running. No adb, no user action.

That is the self-update claim demonstrated rather than asserted: a UI change
reaches a locked phone over Freenet, with no store review and no server.
It is one device on one network and should be read as such, but it is a
measurement rather than a hope.

## 2026-08-01 — node memory is 10x what the README claimed

Starting the long-run watcher immediately produced a correction. The README
had said "node RSS 31 MB" since the first device test. `/proc/<pid>/status`
after a long run hosting real contracts:

```text
VmRSS:   324764 kB     (317 MB resident)
VmHWM:   459460 kB     (449 MB peak)
```

The 31 MB figure was measured minutes after a cold start, before the node
hosted anything. It was true when written and became misleading without
anyone touching it — the worst kind of wrong number, because nothing ever
prompts a re-check.

On this device (7.4 GB, 2.9 GB free) 317 MB is about 4% and survivable. On
a 4 GB phone Android's low-memory killer would eventually take it. That is
a real constraint on "a Freenet node on your phone", and the README now says
so with both numbers rather than the flattering one.

**Battery drain remains unmeasured, and could not be measured tonight**: the
phone is USB-powered because adb needs the cable, so every sample reads
`plugged: 1`. Measuring drain needs a run with no cable, which means no adb,
which means the phone has to log it itself or the user has to read it off
afterwards. Recorded as still-open rather than quietly skipped —
`scripts/battery-watch.sh` is sampling CPU, RSS and Doze survival, which the
cable does not affect, and charging is full-contribution mode so it is the
interesting worst case for CPU.

## 2026-08-01 — the duty cycle was cosmetic, and CPU is 41% of a core

The long-run watcher answered a question nobody had asked yet.

**Measured, dozing, over 30 minutes:** ~12 400 CPU ticks per 300 s sample,
five samples running, i.e. **~41% of one core, sustained**. RSS drifted
324 MB → 282 MB (something is evicting, which is at least working).

Then the reason, which is worse than the number. `NodeService.startNode`
computed `shouldContribute()` and used it for **exactly two things**: the
state enum and the notification string. The command line was byte-identical
either way. So:

- a phone showing "On the network · saving battery" ran precisely as hard as
  a contributing one;
- the README's duty-cycling section described behaviour that did not exist;
- and the app's central ethical claim — *users pay with resources, and we do
  not treat those resources as free* — was being asserted by a notification
  rather than implemented.

This is the same shape as everything else found today: the code computed the
right value and did not act on it, and nothing failed. The value was even
logged. It just never reached the process.

**Fixed:** off-condition the node now runs with
`--max-number-of-connections 5 --total-bandwidth-limit 200000`, against
defaults of 20 and 3 MB/s. Not zero connections — a node that cannot route
has silently left the network, and the user would stop receiving messages
with nothing to explain why.

**Still unsolved, and now stated in the README rather than implied away:**
41% of a core while *contributing* is a lot for a background service. The cap
addresses the off-condition only. Bandwidth and connection limits do not
obviously explain that much CPU, so the next question is what the node is
actually doing — and that is a measurement, not a guess.

## 2026-08-01 — what the 41% is actually doing

Followed the CPU to its source rather than guessing. The node's own logs:

```text
[RATE LIMIT per-callsite] summarize_contract_state: dropped 1780 in last 30s
                                                   (cumulative: 409369)
```

Against a 30/sec cap, that is **~89 calls per second, sustained**, on an idle
node. Each one runs `summarize_state` inside WASM.

**Our contract made it much worse than it had to be.** `summarize_state`
decoded the entire `CellState` — up to 500 records, each with a 16 KiB
thumbnail — purely to collect the map keys. At 89 calls/sec that is megabytes
of deserialisation per second, thrown away except for a set of ids.

Fixed with `summary_from_bytes`, which decodes the keys and skips each value
with `serde::de::IgnoredAny`, so thumbnails are scanned rather than
allocated. A test asserts it produces exactly the same summary as the full
decode — if the two ever diverged, peers would exchange deltas against a
summary that does not describe their state, and convergence would fail in a
way that looks like random message loss.

The call rate itself is upstream's. Drafted as
[`docs/upstream-issues/summarize-contract-state-call-rate.md`](upstream-issues/summarize-contract-state-call-rate.md)
— **not filed**: it needs a minimal reproduction against a stock node first,
because "our app is slow" is not an upstream bug report.

Worth noting why this went unseen: on a desktop, 41% of a core is invisible.
It is only a phone that makes this the difference between a viable background
service and one users uninstall.

## 2026-08-02 — memory and CPU are climbing, cause not yet established

The `summarize_state` fix did **not** reduce measured CPU. Both numbers went
the other way over 90 minutes:

```text
23:05   41.6% of one core   rss 275 MB
23:20   49.0%               rss 292 MB
23:35   61.0%               rss 345 MB
23:50   59.4%               rss 434 MB
00:10   53.5%               rss 580 MB
```

Now `VmRSS 604 MB`, `VmHWM 685 MB`, 16 threads, still rising. On a 7.4 GB
device with 2.6 GB free this is survivable; on a 4 GB phone it would not be.

**The honest position: I cannot yet attribute this.** The inflection at
~23:15 coincides exactly with when I began republishing frequently. Across
tonight I published roughly a dozen UI contract versions (2.7 MB each) and
ran the mainnet smoke suite several times, creating presence, inbox,
moderation and album contracts. A node that retains those is doing what it
was asked to do, and my activity is nothing like a real user's.

So there are two live hypotheses and no evidence separating them:

1. the node accumulates contract state without shedding it, and would do
   this to any user over time;
2. I created an unusual number of contracts in 90 minutes and the node is
   holding them exactly as designed.

**The experiment that distinguishes them costs only patience**: stop all
publishing and testing, leave the node completely idle, and watch. If RSS
keeps climbing with no new contracts arriving, it is (1). If it plateaus, it
is (2) and the earlier numbers were an artefact of how I was working.

Recording this before knowing the answer, because the temptation to write
"probably just my testing" and move on is exactly how the 31 MB figure
survived for weeks.

The `summarize_state` fix stands on its own merits regardless — decoding
16 KiB thumbnails to collect map keys was wasteful at any call rate — but it
is not yet shown to have changed anything measurable, and should not be
described as though it had.

**The quiet experiment answered it.** Thirty minutes with no publishing, no
smoke runs, node completely idle:

```text
00:15   53.0% core   rss 614 MB    <- peak, still mid-churn
00:20   55.8%        rss 438 MB
00:30   53.3%        rss 385 MB
00:40   48.7%        rss 368 MB
```

**Memory does not leak.** RSS gave back ~250 MB within 20 minutes of the
activity stopping. It grows with hosted contracts and releases them —
hypothesis (2). The alarming climb was my own churn: a dozen 2.7 MB UI
versions plus repeated smoke suites in 90 minutes, which is nothing like a
user's pattern. Good news, and worth the half hour of patience to establish
rather than assume.

**CPU did not follow it down.** It settled at ~49–53% and stayed there with
the node doing nothing anyone asked for. So the earlier 41% figure was, if
anything, generous, and the cost is a genuine baseline rather than an
artefact of how I was working. That is now the single biggest unsolved
problem with LKNG on a phone, and the README says so in those terms.

It also means the `summarize_state` optimisation, while correct, was not the
bottleneck. The call rate is upstream's and the draft issue stands.

## 2026-08-02 — halving the subscription count

With the CPU baseline established at ~50% of a core and the call rate
upstream's to fix, the lever we actually control is **how many contracts the
phone hosts at all**. Each subscription is summarised continuously.

`watch_set` was 9 cells × 2 epochs = **18 presence subscriptions**. Now 10.

The cut is chosen by what a subscription is worth, not just what it costs:

| | kept | why |
| --- | --- | --- |
| home, current | yes | the main event |
| neighbours, current | yes | someone a metre across a boundary is as nearby as anyone in your own cell |
| home, previous | yes | or the grid empties every six hours at rollover |
| neighbours, previous | **no** | stale *and* further away — the least valuable thing in the grid |

Three tests hold each half of that, and the pre-existing test asserting 18
was updated rather than deleted, with the reasoning in place so the number
is not silently "corrected" back later.

Whether this moves the measured CPU is not yet known — the phone has to pick
up the new contract first, and saying it will would be exactly the mistake
made about `summarize_state` an hour ago.

## 2026-08-02 — the CPU is the node's, not the app's

The subscription cut did not move the number either (47–54% across the
following half hour). Checking *why* produced the useful fact:

**No WebSocket client was connected during any of these measurements.**
`/proc/net/tcp` shows nothing on the node's port; the app's WebView was not
running, only the foreground service. The work is spread across ~8
`freenet-main` tokio workers.

So the ~50% is the node's own behaviour with no client, no user, and a
dozing device. Neither of our two optimisations could have changed it, and
describing them as performance fixes would have been wrong — they are
correct on their own terms and that is all.

This also makes the upstream issue much stronger than "our app is slow". The
draft is rewritten around the isolated measurement: no client, Doze, 100
minutes, ~51% mean, non-decaying, `summarize_contract_state` at ~89 calls/s.
It states plainly what we ruled out on our side, and what is still missing —
a minimal reproduction against a stock node hosting one trivial contract,
which is the first thing a maintainer would ask for and which we have not
built.

Still not filed, for that reason.

**A desktop comparison, which weakens the story and is worth saying anyway.**
The same node on x86_64 (Xeon E5-2630 v4) uses **18.5% of one core**, not
50%. So the node is likely doing a roughly constant amount of background
work, and the difference is core throughput — a 2.02 GHz phone core with far
lower IPC versus a desktop Xeon.

That is a materially weaker claim than "phone-specific bug", and the upstream
draft now says so in those words. The problem does not disappear: a constant
background cost that eats half a phone core still decides whether a node can
live on a phone. But it looks like the node's ordinary idle cost measured on
weaker hardware, not a mobile-only defect, and a maintainer should be told
that rather than left to discover it.

Also spotted: the desktop node holds **7 leaked client WebSocket
connections** and 1.2 GB RSS after an evening of `fdev` invocations. That is
a development artefact — a user's node has one client — but it is the same
shape as the self-inflicted DoS fixed earlier with `Demux::close()`, this
time from tooling rather than our examples. Noted, not chased tonight.

## 2026-08-02 — the reproduction, and what it actually shows

`scripts/repro-idle-cpu.sh`: fresh node, empty data dir, no contracts, no
clients, 90 s to join, then sampled identically to everything else.

```text
fresh node   (0 contracts, 0 clients):    0.4% of one core,   42 MB
established  (many contracts):           27.4% of one core, 1192 MB
```

**A node hosting nothing costs nothing.** So the idle cost is not a baseline
the node always pays — it is roughly proportional to hosted contracts. The
fresh node logged zero `summarize_contract_state` lines, which fits.

Three things follow.

**The upstream report is now sharp and actionable.** Not "a node is slow on a
phone", which invites the true and unhelpful answer that phone cores are
slow, but "per-contract idle cost is significant and mostly recomputes an
identical summary". It has a reproduction script that runs in four minutes.

**The desktop/phone gap makes sense.** Both nodes host a lot; the phone has
a weaker core. Nothing mysterious remains.

**And it names LKNG's own lever.** What a phone costs its owner is set by
*how many contracts it hosts*. That is a product decision we control, not
something to wait upstream for. Cutting the watch set 18 → 10 was the right
direction taken for the wrong reason — I did it hoping to reduce CPU on a
node whose app was not even connected. The reasoning was wrong; the change
was right, and now for a reason that is measured.

Worth stating plainly: three consecutive attempts to explain the CPU were
wrong (the summarize cost, the subscription count, a phone-specific defect).
Each was disproved in minutes by measuring instead of arguing. The pattern
of the whole session holds — reading and reasoning produced confident wrong
answers, and running something produced the right one every time.

## 2026-08-02 — the smoke suite cried wolf, and a tired node

A full regression run gave 4/5, with `album_share` failing twice on
`put timed out`. Run alone immediately afterwards, it passed completely.

So the script's failure message — *"A failure here is a broken write path,
not a flaky test"* — was wrong, and wrong in the way that matters: a warning
which fires on environmental noise is one people learn to skip, and the cost
lands precisely when it is the real kind.

Now it classifies. A timeout is the network declining to carry a write and
is reported as such, with the instruction to re-run the example alone before
concluding anything. An assertion or verification failure keeps the strong
wording and exits non-zero, because none of these examples fail that way for
environmental reasons. Three attempts rather than two, since two was
demonstrably not enough.

**And the node is tired.** The desktop node after ~5 hours: **1 220 MB RSS,
27% of a core, 14 client WebSocket connections** left by `fdev` invocations
that never closed. That is almost certainly why writes started timing out —
it is the same self-inflicted congestion fixed earlier with `Demux::close()`,
arriving this time through tooling rather than our own examples.

Not a user-facing problem: a real install has one client and does not run
`fdev` fifteen times an evening. But it is a real hazard for anyone
*developing* on Freenet, and worth knowing before spending an hour
diagnosing "the network" when the answer is a leaked socket.

## 2026-08-02 — a correction, and a better measurement

**Correction.** I recorded that "a node restart loses its hosted contracts",
having seen a restarted node at 4 MB and 0% CPU. That was wrong: the node had
not started at all. `pgrep -f 'freenet network'` was matching my own shell
command line, so a dead node looked like a live idle one. Contracts persist
fine — a correctly restarted node loads them eagerly and reaches ~900 MB RSS
within 31 seconds.

Nearly published a false finding on the strength of a pattern that matched
the wrong process. The tell was there — 4 MB is not a running Freenet node —
and I read it as evidence instead of as an implausibility.

**The corrected measurement is more useful than the wrong one:**

| node state | CPU | RSS |
| --- | --- | --- |
| empty data dir, 0 contracts | 0.4% | 42 MB |
| same contracts, 2 min after restart | 7.1% | 1 040 MB |
| same contracts, 5 h uptime | 27.4% | 1 192 MB |

So there are **two** effects, not one. Contracts are necessary — an empty
node costs nothing and logs no summarisation at all. But cost also grows
with *uptime on an unchanged contract set*: 7.1% → 27.4% over five hours,
during which the contract set barely changed.

The second effect is the one that matters for a phone, because it decides
whether the node is viable over a day rather than an hour, and it is the one
we cannot explain. The upstream draft now asks about it directly rather than
implying contract count is the whole story.

Practical upshot for LKNG in the meantime: **the node benefits from being
restarted**. The duty cycle already stops and starts it on charge and network
transitions, which turns out to be worth more than it was designed for.

## 2026-08-02 — the uptime curve, and a second wrong attribution

Same node, same data directory, sampled at increasing uptime:

```text
 2 min:   7.1% of one core, 1040 MB
34 min:  12.6% of one core, 1218 MB
 5 h:    27.4% of one core, 1192 MB
```

CPU roughly quadruples over five hours on an unchanged contract set. RSS is
broadly flat, so it is not memory pressure. This is now three points on a
curve rather than two points hours apart, which is what the upstream report
needed.

**Second correction of the night.** I attributed the node's 14 open client
connections to "leaked `fdev` invocations". After a clean restart with no
`fdev` runs at all, there are still 7 — and their remote ends have **no
owning process**. They are dead sockets the node is not reaping, not
anything our tooling left behind.

That connects to something already seen: earlier in development this node
reached `LISTEN 129 128`, an exhausted accept backlog, and refused new client
connections — which presented as "the network is down". We fixed our side
then (`Demux::close()`); the accumulation continues without us.

On a phone the consequence is specific and bad: a long-lived node that stops
accepting clients is an app that has silently lost its own node, with a
running foreground service insisting everything is fine.

Both corrections tonight came from the same habit — I explained a number
with the most available story rather than checking it. "fdev leaked these"
was plausible, matched the timeline, and was wrong. One command to the
process table settled it.

## 2026-08-02 — a health check for a failure that lies

`NodeService` now probes the client port every two minutes and restarts the
node after two consecutive failures.

The failure it exists for is one already observed: the node stops accepting
client connections while remaining alive. Process running, foreground
notification cheerfully reporting "On the network", app unable to reach its
own node. The user sees an empty grid and no messages and has no way to tell
that anything is wrong — and neither would any liveness check that asks
whether the process exists.

Two consecutive failures rather than one, because a single failure can be a
busy moment during startup or a network transition, and restarting on it is
how a health check becomes a restart loop worse than the fault. Two minutes
apart is far below the timescale on which the backlog fills (hours) and well
above any transient.

`onDestroy` clears the flag and interrupts the thread before stopping the
node — otherwise the check sees the port close and helpfully restarts a
service the user has just stopped. A node that will not stay stopped is
worse than one that will not stay running.

Verified by a clean Kotlin compile and installed on the device. Not yet
verified in the failure case: forcing the backlog to fill takes hours, and
claiming it works without having seen it fire would be exactly the mistake
this log keeps recording.

## 2026-08-02 — a fourth point, and the third correction on the same topic

```text
 2 min:   7.1% of one core
34 min:  12.6%
70 min:  28.6%   <- already at the five-hour level
 5 h:    27.4%
```

**It is a warm-up, not a leak.** CPU ramps over roughly an hour, reaches
~28%, and stops. The 70-minute and five-hour readings agree. And the dead
client sockets sit at exactly 7 at both 34 and 70 minutes — not
accumulating either, within this window.

So both alarming readings from earlier tonight were wrong, and wrong the same
way: **a curve drawn through two points is not a curve.** I described
"CPU quadruples over five hours" and "dead sockets accumulate" from samples
an hour apart, and a third and fourth point turned both into something
ordinary — a service warming up to steady state.

That is now three consecutive corrections on this single topic (contracts
don't persist → they do; fdev leaked the sockets → it didn't; unbounded
growth → a plateau). The common cause is not carelessness with any one
measurement; it is reaching for the shape of an explanation before there
were enough points to constrain it.

The upstream draft is corrected accordingly and its second question is now
the honest one: *what is the node doing during the one-hour ramp?* — rather
than a claim of unbounded growth that would have been refuted by the first
maintainer who ran it for ninety minutes.

The `summarize_contract_state` rate and the 0.4%-vs-28% contract dependency
are unaffected: both were measured directly rather than extrapolated.

## 2026-08-02 — the publish script could make the source lie

Final verification found source and nodes disagreeing: both served an older
build than `web/src/main.rs` claimed. Not a stale node — a stale *source*.

`publish-ui.sh` stamps the marker into the source **before** building, which
is necessary (the marker has to be in the wasm it verifies). But two publish
attempts had failed against a restarting node, and each left a fresh marker
in the source describing a build that exists nowhere. The next comparison
then reads as "the node is stale", which is exactly backwards, and is the
diagnosis that cost most of yesterday.

The script now tracks whether any publish succeeded and **fails loudly** if
none did, naming the marker that is now orphaned in the source.

A tool built to stop one class of silent inconsistency had quietly created
another. That it took a routine verification pass to notice is the argument
for running one.

## 2026-08-02 — multiple profile photos, and the app can now update itself

**Photos.** `PhotoRef` already carried `is_primary` and a hash — but the hash
referred to chunked blobs that were never written, so a profile could name a
primary photo it had no way to display. Photos are now stored inline
(96 KiB each, 512 KiB total, 8 max) and the primary is chosen by the user.

Decisions worth their weight:

- **Exactly one primary is validated, not assumed.** Two makes two clients
  disagree about whose face a profile shows; zero makes a profile with
  photos display none. Both are tested.
- **The aggregate cap exists as well as the per-photo one**, because eight
  legal photos is 768 KiB and the total is what a replicating peer pays.
- **Adding a photo does not silently become your main one.** Only the first
  does. Changing the face strangers see in the grid should be a decision,
  not a side effect of uploading a second picture.
- **Changing the primary re-derives the 16 KiB tile image.** Without it the
  grid keeps showing the old face while the profile shows the new one — and
  the grid is the one strangers see.
- **The primary choice is inside the signed payload**, so a peer cannot swap
  which photo is someone's main one without breaking the signature.
- `to_profile_photo` and `to_album_photo` now share one encoder. Two
  near-identical copies is how one of them quietly stops stripping EXIF.

**Self-update, closed.** The user reported the footer reading `b45941` while
the current build was `b56492`, and force-stopping the app fixed it. So the
node fetches new versions correctly — measured at ~15 minutes to a locked
phone — but the running WebView keeps whatever it loaded at startup, and a
user who leaves the app open sees an old build forever with no way to know.

That matters more here than in most apps: there is no store review in this
path, so a security fix reaches people only when their app reloads, and
"force-stop the app" is not a remedy anyone applies to a bug they cannot see.

The app now re-fetches its own page every ten minutes and reloads if the
content-hashed asset names have changed. It reloads rather than prompting:
the draft, messages and identity are all in storage, so a prompt buys
nothing except the chance to decline a fix.

## 2026-08-02 — profiles were never published either

Same shape as presence, found the same way — by asking what a feature does
end to end rather than whether its pieces exist.

The profile editor wrote a draft to device storage and **made no network
write at all**. A tile carried a headline and a 16 KiB thumbnail; bio,
photos, age and position went nowhere. Someone could fill in a profile, see
it rendered back, and be the only person alive able to read it. The multiple
photos added an hour earlier were, until now, a private slideshow.

`publish_profile` signs the body with the durable key and writes it via the
same seed-then-update the other contracts use. Sequence numbers come from a
stored counter rather than a clock: two devices with skewed clocks would
otherwise fight, and each publish would have to beat the last by luck.

Publishing happens on **Done**, not per keystroke. Each publish is a full
contract state pushed to peers, and a profile saved per-character would be
hundreds of writes for one edit.

**What a stranger can actually reach, stated precisely.** A profile's address
derives from a verifying key, and a tile carries an *epoch* key — so the
profile reachable from a tile is the one published under that epoch key, and
it stops being findable when the epoch turns. That is a deliberate limit, not
an oversight: a durable profile address reachable from a public tile would
make the tile a permanent handle, which is the exact linkability the epoch
design exists to prevent.

Profiles are fetched when a tile is opened, not for every tile in the grid —
fetching 500 profiles to render nine thumbnails would be the app treating the
network as free — and verified before anything is rendered, since an
unverified profile is whatever the last writer put at that address.

**Proven on mainnet** (`profile_photos`): a profile with three photos
published, fetched, verified — 18 628 bytes. All three photos identical to
what was sent. **The primary is the one the owner chose, not the first in
the list**; changing it and republishing is visible to a reader; and a
tampered primary fails verification, so a peer cannot flip whose face a
profile leads with.

The test deliberately publishes with the *second* photo primary. Had the
reader simply taken `photos[0]`, every other assertion would still have
passed with primary = 0 and the bug would have shipped — which is exactly
how photos looked finished twice before while being device-local.

Added to the smoke suite, now six paths.

## 2026-08-02 — favourites and notes were built and never wired

`AddressBook` — favourites, private notes, blocks, export/import, all
unit-tested — has been in `lkng-app` since early on and appeared **nowhere in
the UI**. The user asked for both features explicitly weeks ago. The library
was complete; the button did not exist.

That is the fourth instance of the same shape today (presence publishing,
profile publishing, photo storage, and now this): the hard part written and
tested, the last wire never connected, and nothing failing to say so.

Now wired: Save on a profile, a private note under it, both persisted to
device storage only. The copy says plainly that notes never leave the device
— not to them, not to us, not to the network — and travel only inside a
backup file.

**One honest limitation surfaced in the UI rather than discovered later:**
favourites key off the verifying key on a tile, which is an *epoch* key. So a
favourite does not survive the epoch turning over. Fixing that properly means
favouriting a durable identity, which is only available after someone shares
their profile — the natural place for it, and the next step rather than a
silent compromise now.

## On "see who viewed you"

Not built, and not going to be as Grindr has it. Fetching a profile here is a
contract GET: the owner cannot observe it, and no server exists that could.
Reproducing the feature would mean building the surveillance apparatus this
project exists to refuse, and PLAN.md said so before any of this was written.

The Taps screen already covers the underlying want — *someone noticed me* —
with the person choosing to send it.

## 2026-08-02 — grid filters, the fifth unwired library

`GridFilter` — age bands, positions, headline, same-cell-only, with tests —
appeared **zero times** in the UI, and `matches` was private, so the type
could be constructed and never applied. It was constructed nowhere either.

Fifth instance today of the same shape. At this point the pattern is worth
naming as a property of the codebase rather than a run of coincidences:
**this project has consistently built the hard, testable half of a feature
and left the last wire unconnected**, and because nothing fails when a
library goes unused, every one of them looked finished from the inside.

The filter bar now sits above the grid, matching the layout in the
reference screenshots: position chips, age bands, my-area-only.

Two things the copy says out loud, because both are choices:

- **Filters run on this device over tiles already fetched.** Nothing about
  what someone is looking for is transmitted — there is nobody to transmit it
  to. On a centralised service in this category, that query is the single
  most valuable thing logged about a person.
- **People who stated nothing are hidden by a filter, not quietly included.**
  A filter that sweeps in non-answers lies to the person filtering, and puts
  the people who withheld information in front of exactly the audience that
  filtered for it. Tested.

## 2026-08-02 — the audit, and a bug I shipped an hour earlier

Rather than keep finding unwired libraries one per iteration, I diffed every
public API in the workspace against what the app calls. It surfaced the
worst one so far.

**Blocks did not persist.** The UI kept blocked pseudonyms in an in-memory
`use_signal(Vec<...>)`. Someone could block a person harassing them, close
the app, and find them back in the grid — with a Block button that had
looked like it worked. `AddressBook` has had a `blocked` field since the
beginning with **no accessors at all**, which is why nothing used it.

Blocks now live in the address book, and merging is a **union, never a
replacement**: restoring a backup must not un-block someone blocked since
that backup was made. Getting that backwards would silently undo a safety
decision at the exact moment a user is recovering onto a new phone.

**And the test found a bug I had shipped an hour before.** `AddressBook` is
keyed by `[u8; 32]` and `Vec<u8>`; JSON map keys must be strings, so
`serde_json::to_string` *fails* on this type. My `save_book` wrote JSON
behind an `if let Ok(...)` — so every save silently stored nothing, and
favourites and notes were lost on every reload while the UI displayed them
working perfectly.

That is the same failure mode this log has been cataloguing all session, and
this time I wrote it. Persistence is now CBOR, and a test asserts the JSON
attempt **fails**, so nobody can "simplify" it back:

```rust
assert!(serde_json::to_string(&b).is_err(),
        "...saving as JSON silently stores nothing");
```

The lesson holds in both directions: the audit found the block bug because I
went looking systematically, and the block bug's test found the persistence
bug because it round-tripped through the real path instead of asserting on a
value in memory.

## 2026-08-02 — account deletion

`sign_profile_deletion` existed, was tested, and was called from nowhere —
while `store-listing.md` carried "account deletion" as an unticked Play
requirement. Another entry in the same pattern, found by the same audit.

Now wired, and the ordering is the part that matters: **tombstone first, wipe
second.** Wiping first would destroy the only key able to sign the tombstone,
leaving the profile permanently live and unowned — an account nobody can
delete because the person who could is gone.

The tombstone's sequence is set well past the last live one, so a peer
holding several older bodies cannot replay one over the deletion.

**The copy refuses to say "your data has been deleted."** It says what is
true: the profile is marked deleted, this device is wiped, and tiles already
copied to other phones stay there until they expire — anything anyone saved
is theirs. Nothing on this network can be recalled, *not by us, because there
is no us*. An app whose entire argument is that it does not lie to its users
does not get to start at the delete button.

Two taps rather than a confirm dialog, because it is the only irreversible
control in the app and it lives in a list people scroll.

## 2026-08-02 — photos never worked on the phone at all

The user, three times over two days: *"I still can't add photos."*

**`WebChromeClient.onShowFileChooser` was never implemented.** Without it an
Android WebView swallows `<input type="file">` entirely — no picker, no
error, no console message. The tap does nothing.

So every photo feature was unreachable on the device: the profile photo, the
multi-photo gallery, the primary selection, album photos, and the backup
restore file input. All of them built, unit-tested, published, verified on
mainnet — and impossible to trigger on a phone. They worked perfectly in a
desktop browser, which is where I kept checking.

This is the sharpest version of the session's lesson. Every piece of evidence
I gathered was real: the tests passed, the contracts round-tripped on
mainnet, `to_thumbnail` genuinely strips EXIF, the primary flag genuinely
survives a signature. **None of it touched the one path a user takes.** I
verified the parts I had built and never the thing the user does, and the
user told me three times.

Two details in the fix that are failure modes of their own:

- **A `ValueCallback` left un-called wedges the input permanently.** The
  WebView believes a chooser is still open and ignores every later tap for
  the life of the page — indistinguishable from having no chooser at all.
  So cancellation answers with `null`, and a new request answers any stale
  one before replacing it.
- **The launcher is registered as a field**, not lazily inside the callback.
  The Activity Result API requires registration before the activity starts;
  doing it on first use throws — and only when a user first taps "add a
  photo", which is precisely the moment this bug already ruined once.

No storage permission is requested. `ACTION_GET_CONTENT` through the system
picker grants read access to the single chosen file. Asking for
`READ_MEDIA_IMAGES` to upload one photo would mean requesting the entire
gallery to read one file, from an app whose argument is that it takes only
what it needs.
