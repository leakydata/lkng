# LKNG — Final Plan

A Grindr-class geosocial app with no company in the middle: free, open source, no ads, no data harvesting, carried by the modern Rust Freenet network with the node on the user's own phone.

**The bar:** install from an app store, set up a profile like any dating app, see the grid. Under two minutes, zero crypto vocabulary, nothing to configure. Everything below serves that.

---

## Context

Grindr's free tier is deliberately degraded and ad-saturated; its paid tier costs ~$300/year and is partly surveillance sold back to the user; its central database has produced documented real-world harm (location-data sales, trilateration, entrapment). The paywall and the harvesting are one business model seen from two sides — LKNG removes both at once by having no company in the middle.

This plan is grounded in a full read of the Freenet ecosystem source — `freenet-core`, `mail`, `delta`, `river`, `raven`, `freenet-git`, `harvest`, `atlas`, `ghostkeys`, `freenet-scaffold`. Nearly every component has a working reference implementation to copy, and several of LKNG's hardest problems turn out to be already solved upstream. The plan's job is assembly and honesty, not invention.

## Decisions

| Decision | Choice |
| --- | --- |
| Substrate | Freenet behind `lkng-transport` trait; mock backend keeps UI work unblocked |
| Node model | Bundled **unmodified** freenet-core binary, child process, loopback WebSocket |
| Contribution | Duty-cycled: full peer while charging on Wi-Fi, leaf otherwise. Users pay with resources, not money. |
| Distribution | **Google Play AND F-Droid AND direct APK.** The AGPL concern that ruled Play out was wrong — see Licensing. |
| First run | **Warm start**: official gateway WebSocket for an instant first session, silent handoff to the embedded node |
| Identity | Invisible keypair (delegate + Keystore). Recovery via passphrase-encrypted backup stored *on Freenet itself*. |
| Location | Uniform geohash cells (level 5 default); user control = jitter radius (none/~1km/~5km), stable offsets |
| Discovery UX | Grindr-style grid — thumbnail + short headline; durable profile revealed after mutual interaction |
| IP exposure | Accepted: ring topology bounds it to gateway + neighbour set. No VPN, no Tor. |
| Location fraud | Accepted and documented. No feature may depend on a claimed position being real. |
| Cell contracts | Raven's `global-index-shard` pattern (anyone-writes, capped post-merge), parameterized by `(cell_id, epoch)` |
| Conversations | River-style private 2-person rooms, ECIES, random contract IDs |
| Accountability | Harvest's blind-signature feedback tokens exchanged at match time |
| Moderation | Subscribable feeds shaped as Atlas descriptors; baseline feed on by default |
| Signatures | ML-DSA-65 (FIPS 204) — Mail and Raven have converged on it |
| Funding | None. No ads, no paid tier, no monetization plan. Grants (NLnet/OTF) optional later. |
| License | LKNG open source; freenet-core bundled unmodified (AGPL does not infect over WebSocket) |

---

## The first-run experience — the part that makes it a product

Grindr's onboarding: install → age gate → photo + display name → grid. LKNG must match that beat for beat. Three mechanisms, each inferred from working ecosystem code, make it possible:

### 1. Warm start via gateway handoff

A fresh Freenet node needs time to join the ring; a first-time user will not stare at a spinner. But Delta's client derives its WebSocket URL from its serving location and speaks the **identical client API** whether the endpoint is a local node or a public gateway — and try.freenet.org proves gateway-mediated app access works on phones today.

So: on first launch the app connects to an official gateway **instantly** — the user browses the grid and builds a profile within seconds — while the embedded node bootstraps in the background. When the local node is ready, the app switches endpoints mid-session. Same client code, two sockets, zero user-visible difference.

Honesty requirement: during warm start the gateway sees your IP and query pattern, exactly as Grindr's servers always do. The handoff removes that. A privacy setting offers "never use gateway" for those who prefer a slower first session; the default favors the two-minute experience.

### 2. Invisible identity, real recovery

Account creation generates an ML-DSA keypair inside the identity delegate, wrapped by Android Keystore. The user never sees a key, phrase, or hex string — they see "pick a name, add a photo."

Recovery is the hard half, and the network itself solves it: the identity bundle (freenet-git's `crates/identity` encrypted-bundle format, pinned wire tests) is encrypted under an Argon2id-stretched passphrase and **published as a small Freenet contract at a key derived from that passphrase**. A new phone: install → type passphrase → fetch bundle → decrypt → you're back, profile and conversation keys intact. Signal-style recovery with no server — the network is the cloud backup.

- Backup is opt-in at onboarding ("set a recovery passphrase — without it, losing your phone loses your account"), re-prompted later, never blocking.
- The backup contract is subject to eviction like everything else; the app re-publishes it on each open, and it's tiny (one contract, no chunking).
- A weak passphrase is brute-forceable offline by anyone who guesses the derivation scheme; Argon2id parameters set aggressively, passphrase strength meter required.

### 3. The store listing

The earlier decision to skip Google Play rested on my AGPL misreading, since corrected (see Licensing). With that gone, the remaining Play obstacles are policy work, not blockers — Grindr itself is on Play, so the category is admissible. LKNG ships to **Play (primary reach), F-Droid (trust anchor + reproducible builds), and direct APK (censorship hedge)**. The three-channel spread means no single delisting kills distribution — which, for this user base, is itself a safety feature.

Play requirements, all already in the plan for ethical reasons: in-app reporting and blocking, UGC policy, moderation process, 18+ age rating and gate, privacy policy, foreground-service disclosure, account deletion (local wipe + tombstoned profile — with the honest caveat that replicated ciphertext cannot be recalled). The Data Safety form is short: no data collected by the developer, because there is no developer server. That is a listing differentiator, not just compliance.

### 4. The app updates itself over Freenet

Every ecosystem app ships its UI as a **web-container contract** (`web_container_contract.wasm` + a published contract ID), with Mail and Raven adding a **facade contract** for version indirection. LKNG does the same: the WebView UI loads from the local node's contract store and updates by publishing a new UI version to the facade — signed, verified, no store review cycle for UI/logic changes. The native shell (node runner, Keystore, permissions) still updates through the stores. Result: fast iteration and a UI that cannot be taken down, with native-layer changes on the slower store cadence.

---

## What the ecosystem provides (findings from source)

### Licensing — the correction that reopens Play

freenet-core's `LICENSE.md`: an app that *"communicates with a Freenet node over a network protocol (such as HTTP or WebSocket)"* is *"not considered a derivative work"*, and *"simply bundling or distributing the unmodified freenet-core binary alongside your app does not trigger the AGPL's copyleft requirements."* All app repos and `freenet-stdlib` are LGPL-3.0. LKNG's own license is a free choice; bundling the unmodified core as a child process keeps everything clean. Patching core for Android makes that patch AGPL — ship its source and upstream it, which we'd want anyway.

**License warning:** `harvest` and `ghostkeys` have **no license file**. Patterns may be studied, code may not be copied. File upstream issues asking for licenses in Phase 0.

### Eviction — the sleeper constraint (Gate 1)

`docs/design/hosting-eviction.md` (settled 2026-07-08): peers shed lowest-demand contracts first, ordered by `(local_subscription_count, downstream_subscriber_count, recency, key_bytes)` ascending. *"Subscriptions are not a pin"*, and *"a fewest-subscriber newcomer is simply the one evicted."*

**A new user's profile has one subscriber — themselves. It is structurally first to evict.** In a dating app the users who vanish are the new and the unpopular: a cruel bootstrap failure. Per-contract cost ~1 MB memory / ~650 KB storage, budget scales with RAM, so phones host little. freenet-git's answer (owner keeps a node subscribed + `rescue` cron) doesn't transfer — phones are off most of the day.

Mitigations, resolved at Gate 1: shared per-cell presence contracts accumulate subscribers naturally (discovery is probably fine — it's per-user data at risk); local encrypted storage stays authoritative with republish-on-open (network as transport, not store); duty-cycled charging-hour peers add overnight capacity; volunteer mirror nodes (Tor-relay model) as the explicit, documented fallback — soft re-centralization admitted, not hidden.

### The presence cell is already written — Raven's `global-index-shard`

A fixed-key, anyone-writes contract of self-verifying ML-DSA-65-signed entries: grow-set deduped by content address, **truncated post-merge to newest-N by `(timestamp, id)`** (a total order — no clock in a contract). Two documented subtleties that would each have been a from-scratch bug:

- The cap is deliberately NOT enforced in `validate_state`: *"a transiently over-bound merged state is normal, and rejecting it would break convergence."*
- *"`verify()` proves WHO signed, not that the signer is ALLOWED"* — a reserved `WriterCert` wire slot (vacuous today) lets write-gating arrive later **without rotating the contract ID**. LKNG reserves the same slot in presence records from v1.

### Accountability without identity — Harvest's feedback tokens

Harvest's design (April 2026, working contract code): at transaction time each party blind-signs (RFC 9474) the other's feedback token without seeing it; each then holds a single-use, nonce-deduplicated, unlinkable right to post to the other's **append-only reputation contract** (`ReputationStateV1`: grow-only, RSA-PSS-verified, naturally commutative).

Mapped to LKNG — **tokens exchanged when a conversation is mutually accepted**:

- Only actual interlocutors can leave feedback → **brigading and review-bombing structurally impossible**, no moderator required.
- Feedback is unlinkable to the complainant → reporting a harasser cannot be traced by the harasser.
- One token per interaction → a grudge files once.

Composes with Ghost Keys' economic stake: abandoning a burned reputation costs the donation tier behind it. No centralized dating app can offer this combination.

### The rest of the toolbox

- **`mail/contracts/inbox`** — open-inbox template: owner pubkey in parameters, bounded sender lists (`MAX_VERIFIED_SENDERS`), signed-payload constants, `AddMessages`/`RemoveMessages`/`ModifySettings` enum. LKNG's message-request inbox.
- **`mail/modules/antiflood-tokens`** — working decentralized rate limiter (token contract + generator delegate, tiers, criteria). LKNG's gate on unsolicited contact.
- **`freenet-git`** — `ChunkedPack` for large media (1 MiB chunks, BLAKE3 manifest, four-phase verified commit; per-contract practical limit is *"low single-MB"*); availability math `0.99^176 ≈ 17%` says keep photos few and small; `crates/encoding/canonical.rs` deterministic CBOR (hand-rolled so dependency upgrades can't shift bytes); `crates/identity` encrypted bundles.
- **`delta`** — the profile-contract shape (single-writer, per-item signatures, full `ContractInterface` in ~150 lines); five security lessons in its comments: zero-byte empty deltas (#5072 — a CBOR-encoded empty struct breaks convergence), authenticated key-bound tombstones (LKNG's blocks/deletes/moderation all inherit the attack), placeholder-owner address-squatting (first-write-wins on profile handles), sign-each-item-not-the-state, v2→v1 signature fallback for schema evolution. Plus migration scaffolding (`legacy_contracts.toml`, migration scripts) — **any code or parameter change rotates the contract ID and orphans state; adopt before the first contract ships.** Plus the reconnect-debounced connection state machine.
- **`river`** — 2-person conversation rooms (per-member ECIES cheap at N=2), `SealedBytes`/`PrivacyMode` for public-or-encrypted fields, `#[composable]` scaffold macro (field order is load-bearing), and lesson #145: **deltas must be self-contained** — records referencing unknown entities are silently dropped, so presence records must carry everything needed to validate them.
- **`atlas`** (WIP RFC) — descriptors: signed claims about subjects, explicitly including user profiles, with safety notes and user-controlled trust over competing indexes. LKNG moderation actions are shaped as descriptors so feeds become Atlas-compatible if it lands. Atlas's crawler is also a reminder: public contract state **will** be indexed by infrastructure, not just adversaries.
- **`ghostkeys`** — donation-backed anonymous identities ($1–$100 fixed tiers, blind-signed so donation and key are unlinkable, offline verification). Optional "backed identity" badge in LKNG: the money goes to the Freenet nonprofit, not LKNG, so the no-monetization stance holds. Its delegate's `permissions.rs` and migration rules are the discipline to copy.
- **Delegates generally** — run inside the core, message-passing only, platform-attested origins, consent prompts outside the app sandbox. **A delegate cannot make a contract trust a location claim** (it runs on the user's hardware; spoofing is unpreventable). What it buys: raw GPS never reaches the WebView (the UI asks for "my publishable cell"; jitter+geohash happen inside the delegate), the jitter secret can't be exfiltrated or reset, keys stay out of the web layer. The delegate boundary protects the user from their own UI, not the network from the user.

---

## Location privacy

- **One network-wide cell rung**: geohash level 5 (~4.9 km). Uniform cells maximize every user's crowd and concentrate subscribers per cell contract (which also fights eviction). Compare H3 in Phase 0 (uniform hexagons, clean neighbours) before locking in.
- **User privacy control = jitter radius, not cell size** (none / ~1 km / ~5 km). Per-user cell sizes would put the most privacy-conscious users in the smallest crowds — backwards.
- **Jitter is stable, never resampled**: `offset = HKDF(local_secret ‖ geohash_L4(true_position))` → a fixed offset per user per broad area. Re-randomizing per update lets an observer average samples and recover the true point. Never derive from time.
- **No distance numbers, ever.** Cell membership only. Distance display is what enabled Grindr trilateration.
- 8-neighbour subscription (9 live) handles cell boundaries. Foreground approximate location only; manual location mode; EXIF stripped everywhere; last-active rounded.
- **The linkability ceiling, stated plainly:** the product is a photo grid, faces are biometrics, so cross-epoch unlinkability is *not achieved* — face matching links people across epochs regardless of pseudonym rotation. Rotation still defeats naive key correlation; withholding the durable profile until interaction still limits scrape yield; the teaser is public by design (cell keys would be derivable by any scraper — encrypting to the cell is theater). LKNG is meaningfully better than Grindr on location precision and absence of a central database. It is not "safe," and must never claim to be.

---

## Architecture

```text
Android app (Kotlin / Compose)
├── Foreground service — node child-process lifecycle, duty-cycle policy
├── Android Keystore — identity key wrapping
└── WebView — UI loaded from local web-container contract (self-updating)
        │  WebSocket: gateway (warm start) → 127.0.0.1:<random port> + session token (steady state)
        ▼
Bundled freenet-core binary (unmodified)
        ▼
LKNG application
├── lkng-transport            — narrow trait: publish/subscribe/get, blobs, sign/verify
├── lkng-transport-freenet    — real backend (gateway or local node — same client API)
├── lkng-transport-mock       — in-memory; all UI and contract-logic tests run serverless
├── contracts/  profile · presence · inbox · conversation · media · reputation · moderation · backup · ui-facade
├── delegates/  identity · location (jitter+geohash) · encryption · aft-tokens · local-blocking
└── web/        frontend (Svelte or Dioxus — Phase 0 decision)
```

## Data model

- **Profile** — Delta's shape: one contract per user, single-writer, per-item ML-DSA signatures, sequence + content-hash tiebreak, thumbnail inline (single contract). No location, ever. Most eviction-exposed contract; republished on app open.
- **Presence** — `(cell_id, epoch)`-parameterized Raven index shard: anyone-writes self-verifying records, capped post-merge by `(timestamp, id)`, reserved WriterCert slot for AFT/Ghost-Key gating. Record = per-epoch pseudonym + thumbnail + headline + signature, self-contained (River #145). Epoch rollover replaces pruning (commutative-monoid states can only grow or tombstone). Hard caps on thumbnail bytes and records per cell are load-bearing: this is the hottest contract, the largest state, and every phone in the area pays its bandwidth.
- **Inbox** — Mail's contract: permissionless encrypted envelopes, owner-only processing, bounded sender list, AFT-gated, blocked senders dropped pre-render.
- **Conversation** — random-ID private room (IDs never derived from participant keys — that would let anyone test whether two people talk), River ECIES, rolling on-network window, full history authoritative in local encrypted storage. **Feedback tokens exchanged at mutual acceptance.**
- **Reputation** — Harvest's pattern (until licensed: pattern, not code): per-identity append-only negative feedback, token-verified, nonce-deduplicated. Weighted by Ghost Key tier where present.
- **Media** — thumbnails single-contract; full photos ChunkedPack; resize/recompress/EXIF-strip on device; revocation prospective-only and the UI says so.
- **Backup** — passphrase-derived-key contract holding the encrypted identity bundle.
- **Moderation** — Atlas-shaped signed descriptors; baseline feed on by default; user-managed feed list.
- **UI facade** — points at the current web-container UI version; the self-update path.

---

## Dev environment setup (first action on approval)

Machine state as of 2026-07-31: VS Code 1.131, Node 24, JDK 25, adb, and **`fdev` 0.3.278 already installed**; no rustc/rustup, no Android SDK/NDK, no Rust/Kotlin/Svelte extensions.

Install, scripted and unattended:

1. `rustup` (stable) + targets `wasm32-unknown-unknown` now, `aarch64-linux-android`/`armv7-linux-androideabi`/`x86_64-linux-android` at Gate 2; `cargo-make` (ecosystem builds use `Makefile.toml`).
2. `freenet` node binary (verify — `fdev` present but node binary unconfirmed).
3. Android cmdline-tools → SDK + NDK + accept licenses; **Temurin JDK 21 alongside JDK 25** (AGP compatibility). Android Studio recommended at Phase 5 for emulator/signing work.
4. VS Code extensions: `rust-lang.rust-analyzer`, `vadimcn.vscode-lldb`, `tamasfe.even-better-toml`, `fill-labs.dependi`, `fwcd.kotlin`, `vscjava.vscode-gradle`, plus `svelte.svelte-vscode` if Svelte wins the Phase 0 frontend decision.

## Critical path — first three weeks

The two questions that can kill the project get answered before product code exists. Both start on this machine, today:

1. **Day 1 — run the ecosystem.** Install freenet-core, join the network, use River and Delta. Validates toolchain, docs, and network health in one step.
2. **Week 1 — Gate 1, eviction.** Two local nodes + public network: publish a trivial contract, go offline, measure fetchability over hours/days at varying subscriber counts. A shell script and patience. **If new-user contracts reliably vanish and mirrors can't be stomached, the product doesn't work — find out now.**
3. **Week 1–2 — hello-world contract.** Fork Delta's site contract, rename, build with `fdev`, publish, fetch from a second node. Proves the whole pipeline.
4. **Week 2–3 — Gate 2, Android spike (parallel).** Cross-compile freenet-core for `aarch64-linux-android`, run as child process in a throwaway APK, survive `adb shell am kill`.
5. **Gate 3 — duty-cycle spike.** Drive the node between contributing and leaf on charge/network transitions without corrupting state or thrashing its ring position.

Phase 1 begins only when Gates 1–2 pass. The mock transport keeps frontend work running in parallel regardless. **Every phase ends with something that visibly works and is committed** — momentum is the scarce resource.

## Phases

- **Phase 0 — Research + gates (above).** Also: geohash vs H3; Svelte vs Dioxus; adopt Delta's migration scaffolding; file license-request issues on harvest/ghostkeys; write threat model, privacy model, `anonymity-limitations.md` (reachable in-app, plain language).
- **Phase 1 — Desktop prototype.** Identity delegate (freenet-git bundles), profile contract (Delta shape), canonical CBOR, mock backend, frontend shell. Exit: two desktop users create and fetch each other's profiles; invalid updates rejected; merges order-independent under property-based tests; one migration exercised.
- **Phase 2 — Discovery.** Cells, stable jitter, presence shards, 8-neighbour subscription, epoch expiry, filters. Exit: simulated users appear correctly; a serialization test proves no unjittered coordinate ever enters contract state; stale presence disappears; **adversarial scraping simulation runs and its write-up exists** (including a face-matching-cost estimate).
- **Phase 3 — Messaging + accountability.** Mail inbox + AFT; River encryption; invitation/accept/reject; local blocks; **feedback-token exchange and reputation contract**. Exit: E2E-encrypted chat between two users; third-party subscriber can't decrypt; blocked users invisible; replays rejected; AFT throttles simulated spam; feedback postable only with a banked token.
- **Phase 4 — Media.** Single-contract thumbnails; ChunkedPack photos with four-phase commit. Exit: reliable display, hard caps enforced, EXIF gone, availability measured against the `p^n` math.
- **Phase 5 — Android runtime + warm start.** Foreground service (state machine `STOPPED→…→ONLINE→…`), loopback + session token, app-private storage, duty-cycle policy, gateway→local handoff, backup contract flow, WorkManager polling for message checks. Settings: autostart, contribute-while-charging, Wi-Fi-only, battery floor, storage cap, diagnostics export, identity backup, gateway opt-out. The node never runs without the user knowing; contributing vs leaf is always visible. Exit: two phones, no desktop, full flow; process-kill recovery; endpoint handoff mid-session without user-visible disruption.
- **Phase 6 — Safety + distribution.** Reporting (ethically required, not store-required), baseline moderation feed, age gate, account deletion (local wipe + tombstones + honest caveat), privacy policy/ToS, **Play listing + Data Safety form, F-Droid reproducible build, signed APK**. Age-verification law (UK OSA, US state laws, EU DSA) researched per target jurisdiction and the position documented — a real legal exposure with no clean decentralized answer; not a checkbox.
- **Phase 7 — Hardening + launch.** Battery/storage/network measurement on real hardware; crash recovery; abuse simulations; external security review. **Launch = one city.** Geosocial apps need density per city, not global totals: pick one community (organisation, campus, event), seed it deliberately, only then widen. An empty grid is death regardless of engineering.

## Verification

- **Contract tests:** owner-only updates, signature rejection, sequence conflicts, duplicates, epoch expiry, oversized-payload rejection, replay. Every merge property-based and order-independent — the commutative-monoid requirement makes this the most important test category.
- **The location assertion:** serialize every contract state; assert nothing derives from unjittered coordinates.
- **Eviction sim:** publish, go offline, measure fetchability vs subscriber count. Re-run whenever contract structure changes.
- **Scraping sim:** an adversary subscribed to every cell in a simulated metro for many epochs, attempting movement-history reconstruction. Output is a document with numbers, not a pass/fail.
- **Scale sim:** 20 users/1 cell → 100/9 cells → 1,000 profiles, 10,000 messages. State size, merge time, subscription latency; memory/battery/network on hardware.
- **Android matrix:** Pixel + Samsung, Android 12–15; network transitions, airplane mode, process kill, reboot, battery saver, low storage, permission denial, node crash, corrupted cache, identity restore.
- **Security tests:** WebView origin restrictions, loopback auth, local port scanning, corrupt state, malicious images, decompression bombs, path traversal, log leakage, backup extraction, signature bypass.
- **v0.1 manual E2E:** two phones, no desktop, no gateway (after warm start): identities → presence in neighbouring cells → discovery → encrypted chat → block → report → feedback token; then kill the network mid-conversation and confirm recovery.

## Risks

1. **Eviction kills per-user data** — top risk; Gate 1; volunteer-mirror fallback must be explicit, not emergent.
2. **Freenet is pre-1.0** — four apps, no published node counts, no mobile roadmap (issue #706 closed 2023, uncommented). Transport abstraction hedges; doesn't remove.
3. **Cold start** — existential product risk; one-city launch strategy is the mitigation and deserves gate-level seriousness.
4. **No push without a server** — messages land when the node runs. Foreground service + WorkManager polling + duty-cycle windows; set the "delivery-when-active, like email" expectation honestly.
5. **Play policy volatility** — a dating app with UGC and an embedded P2P node may face review friction; F-Droid + APK channels mean delisting is survivable.
6. **Presence cell size** — hottest contract, everyone's bandwidth; caps are load-bearing.
7. **Face matching defeats pseudonym rotation** — accepted grid consequence; measure it in the scraping sim; state it in-app.
8. **Contract ID rotation orphans state** — migration scaffolding before the first contract, not after.
9. **Gateway warm start is a trust window** — bounded, disclosed, opt-out.
10. **Weak recovery passphrases are offline-brute-forceable** — aggressive Argon2id, strength meter, honest copy.
11. **Deletion doesn't exist** — replicated ciphertext can't be recalled; tombstones + minimal publication + honest UI.
12. **Age-verification law** — unresolved everywhere in decentralized systems; researched per jurisdiction before distribution.

## Funding and ethos

No monetization — the point is what the $300/year crowd gets for free, minus the surveillance. Users pay with resources: duty-cycled contribution while charging on Wi-Fi. Retention shortfalls are met by volunteer mirrors (Tor-relay model), organised as deliberately as code. Ghost Key purchases fund the Freenet nonprofit, not LKNG, so the optional badge doesn't breach the stance. If money is ever wanted: NLnet (≤€50k, short application, calls reopen after summer 2026) and OTF fit exactly; noted as options, not plans. The Grindr-parity list gets audited against the privacy model first — "see who viewed you," server read receipts, and incognito-as-product require the surveillance apparatus LKNG exists to refuse; filters, translation, unsend, and unlimited profiles are client-side and free.

## Recheck at implementation time

Freenet repo structure and APIs, stdlib version, Mail AFT and River encryption details, harvest/ghostkeys license status, Android NDK, Play policy current text, F-Droid reproducible-build requirements, foreground-service and location-permission rules. This document freezes none of them.
