# LKNG

A Grindr-class geosocial app with no company in the middle: free, open
source, no ads, no data harvesting. Profiles, coarse nearby discovery, and
end-to-end encrypted messaging carried by [Freenet](https://freenet.org) —
with the node running on the user's own phone.

**Status: pre-alpha.** The network layer works end to end on mainnet. The
app around it is unfinished — see [What actually works](#what-actually-works).

---

## A Freenet node running on Android

There is no official mobile Freenet build. This repo has one working, and
that part is genuinely usable today independent of the dating app around it.

From the phone's own log:

```text
lkng.node: node started, contributing=true
```

A Samsung Galaxy Z Flip 4 on **Android 16**, running **unmodified**
`freenet-core` as a child process of an ordinary app: 16 distinct peer
addresses, `joined peer` / `connected peers`. It relays for other peers —
the log shows it processing `SUBSCRIBE relay` requests on behalf of
strangers. It is a real peer, not a thin client.

**Memory is the honest problem.** An earlier version of this file said
"RSS 31 MB", measured minutes after a cold start. After a long run hosting
real contracts, `/proc/<pid>/status` reports:

```text
VmRSS:   324764 kB     (317 MB resident)
VmHWM:   459460 kB     (449 MB peak)
```

That is roughly 4% of this device's 7.4 GB, which it survives comfortably.
On a 4 GB phone it would be a different conversation, and Android's
low-memory killer would eventually win. **A node on a phone is not free, and
the number that matters is the one after hours of hosting, not the one after
the splash screen.** Anyone evaluating mobile Freenet should measure their
own; the 31 MB figure was true and misleading at the same time.

### How it is done

**No upstream patches were needed.** `freenet-core` cross-compiles for
`aarch64-linux-android` as-is:

```bash
rustup target add aarch64-linux-android --toolchain 1.94.0   # note the pinned toolchain
NDK=$ANDROID_HOME/ndk/27.2.12479018/toolchains/llvm/prebuilt/linux-x86_64/bin
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$NDK/aarch64-linux-android24-clang \
CC_aarch64_linux_android=$NDK/aarch64-linux-android24-clang \
AR_aarch64_linux_android=$NDK/llvm-ar \
cargo build -p freenet --release --target aarch64-linux-android
```

Four things that were not obvious, and cost time:

1. **`freenet-core` pins Rust 1.94.0** in `rust-toolchain.toml`, so the
   Android std must be added to *that* toolchain. Adding it to your default
   toolchain instead produces a missing-std cascade across every crate,
   which reads like the target being unsupported. It is not.
2. **Ship the binary as `libfreenet.so` under `jniLibs/`, not as an asset.**
   Modern Android will not execute a file from writable app storage (W^X).
   `nativeLibraryDir` is one of the few read-only *and* executable
   locations, and packaging only extracts files matching `lib*.so`.
3. **The node runs as more than one process.** Killing the parent PID
   leaves a child alive and still networking. `Process.descendants()` is a
   Java 9 API Android does not have, so shutdown sweeps on the data-dir
   path, which is unique to the app.
4. **A foreground service is required**, and is also the honest choice — a
   phone silently on a P2P network is not something to hide. The
   notification carries a one-tap **Stop**.

See [`android/app/src/main/java/org/lkng/app/NodeService.kt`](android/app/src/main/java/org/lkng/app/NodeService.kt)
and [`scripts/gate2-device-test.sh`](scripts/gate2-device-test.sh), which
runs the whole check against a connected device.

### The app updates itself over Freenet

The UI ships as a web-container contract, so a change reaches users without
a store review. Measured on 2026-08-01: a new build published to a desktop
node was served byte-identical by the phone's node within about 15 minutes,
with the phone **dozing, locked, and the app in the background** — only the
foreground-service node running. No cable, no user action.

One device on one network, so read it as an existence proof rather than a
performance figure. The native shell (node runner, Keystore, permissions)
still updates through the stores; only the UI and app logic move this way.

### Duty cycling

Users pay for this app with device resources rather than money, so the node
contributes fully only while **charging on un-metered Wi-Fi**, and runs
minimally otherwise. Both conditions are required: charging alone on a
metered hotspot would spend someone's data allowance carrying other
people's traffic.

### Caveats, honestly

- Battery and Doze behaviour over hours is **not yet measured**. Short runs
  look fine; that is not the same claim.
- The APK bundles a cross-compiled node, and there is no upstream Android
  release asset, so it must be re-cross-compiled per release.
- Tested on one device (Galaxy Z Flip 4, Android 16) and an x86_64 build
  exists for emulators. That is a narrow matrix.

If you are working on mobile Freenet, please take any of this. Corrections
welcome — it is one device and one pair of eyes.

---

## The app

Discovery works by **coarse geohash cells**, never distance. Two people at
different coordinates whose jittered positions land in the same ~5 km cell
can see each other; nothing finer is ever computed or published. Distance
readouts are the mechanism behind the documented trilateration attacks on
this category, so there is a test that fails the build if a distance field
appears in the render model.

Other properties, each demonstrated against real network bytes rather than
asserted — the examples in
[`rust/lkng-transport-freenet/examples/`](rust/lkng-transport-freenet/examples/)
run them against mainnet:

- **Tiles are signed by per-epoch subkeys**, so scraping a cell yields
  rotating pseudonyms and no route to anyone's durable profile. The
  encryption key on a tile rotates too — a durable one there would have made
  rotation cosmetic, letting a scraper link a person across every epoch and
  area they ever appeared in.
- **You can message someone from their tile alone**, with no profile
  exchanged and no handshake. ECIES (ephemeral X25519 → HKDF →
  XChaCha20-Poly1305), signed with ML-DSA-65, and an envelope is bound to
  its recipient's inbox so it cannot be replayed into anyone else's.
- **Taps are indistinguishable from messages on the wire**, so nobody
  replicating an inbox can tell who tapped whom.
- **Album photos are ciphertext on the network.** Anyone may fetch an album;
  only people the owner named can open it. The key travels in an inbox
  envelope, so nobody can tell an album was shared, with whom, or that one
  exists.
- **HIV status never touches a public tile** — enforced by a test that scans
  serialized tile bytes for clinical terms, not by convention.
- **Favourites, notes, blocks and sent messages never leave the device**
  except inside the passphrase-encrypted backup. There is deliberately no
  "sent" contract: one would publish a list of everyone you have messaged.
- **Reports are counted by reporter, not by report**, so one determined
  person cannot manufacture the appearance of consensus against someone.

## What actually works

| Component | State |
| --- | --- |
| Freenet node on Android | **Works**, relaying live traffic |
| Contracts on mainnet | **Works** — presence, profile, inbox, moderation, album |
| Encrypted messaging | **Works** — tile → sealed message → read, no profile needed |
| Taps | **Works** |
| Grid UI | **Works**, served *from* Freenet, self-updating |
| Photos on tiles | **Works** — EXIF stripped by canvas re-encode |
| Private albums | **Works** — share, receive, prospective revocation |
| Reporting and blocking | **Works** — blocking is local and immediate |
| Per-install identity | **Works** — sealed by the Android Keystore |
| Profile editor | **Works** |
| Real location | **Works** — coarse device fix, or set by hand |
| Age gate | **Works** — self-declared, and the app says so |
| Onboarding | **Partial** — age gate then the grid; no guided setup |
| Account backup and restore | **Works** — encrypted file, Argon2id-stretched passphrase |
| Battery/Doze measurement | **Missing** — short runs look fine; that is a different claim |

### What "works" means here, and what it does not

Every row above is exercised by a test, and most by an example that runs
against mainnet. But **the app has not been used by two real people on two
real phones**, and until it has, treat "works" as "the mechanism is proven,
the product is not".

**Do not use this as a dating app yet.** The specific gaps that matter:

- losing the phone loses the account unless a backup file was saved; the app
  prompts once you have something to lose, but a prompt is not a guarantee
  anyone acted on it;
- nobody has measured what the node does to a battery over a day;
- a public photo grid is scrapeable and face matching defeats pseudonym
  rotation, which is true of every app in this category and is stated in
  [`docs/anonymity-limitations.md`](docs/anonymity-limitations.md) rather
  than glossed.

## Building

```bash
cargo test --workspace          # 161 tests, no network needed
cd web && dx serve              # the UI against an in-memory backend
cd android && gradle assembleDebug
scripts/publish-ui.sh           # build contracts, then the UI, then publish
scripts/mainnet-smoke.sh        # every write path, against the live network
```

**Use `scripts/publish-ui.sh` rather than running `dx bundle` by hand.** It
stamps a marker into the source and refuses to publish if that marker is
absent from the compiled wasm. That is not ceremony: `dx bundle --release`
re-emitted a previous build's wasm while reporting success, and cost most of
a day of diagnosing a phone that was doing exactly what it was told. The
marker is rendered in the app footer, so "is this device running the new
build?" is answerable by looking at the screen.

The Android build pins JDK 21 in `gradle.properties`. Kotlin 2.0.21 throws on
a JVM version string of `25.0.3` — and because outputs were up to date,
gradle reported BUILD SUCCESSFUL while silently handing back an older APK.

Contracts need the `wasm32-unknown-unknown` target and
[`fdev`](https://freenet.org/dev/). Each contract is its own workspace
root, and `fdev build` needs `CARGO_TARGET_DIR` set — see
[`docs/upstream-issues/`](docs/upstream-issues/).

## Honesty

LKNG is better than centralized alternatives on location precision and on
having no central database. It is **not** an anonymity system: peers see
IPs within the ring topology, a public photo grid is inherently
scrapeable, and published data cannot be universally deleted.
[`docs/anonymity-limitations.md`](docs/anonymity-limitations.md) says
exactly what is and is not protected, in plain language, and is reachable
from inside the app.

[`docs/gate-status.md`](docs/gate-status.md) is the working log — every
design decision, every bug found, and the reasoning behind both.

## License

AGPL-3.0-only. The bundled node is unmodified `freenet-core`; per upstream's
`LICENSE.md`, apps talking to it over WebSocket are not derivative works.
LKNG is AGPL by choice, because software asking for this much trust should
be auditable and stay that way.
