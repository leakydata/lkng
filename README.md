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
addresses, `joined peer` / `connected peers`, node RSS **31 MB**. It
relays for other peers — the log shows it processing `SUBSCRIBE relay`
requests on behalf of strangers. It is a real peer, not a thin client.

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
asserted:

- **Tiles are signed by per-epoch subkeys**, so scraping a cell yields
  rotating pseudonyms and no route to anyone's durable profile.
- **Profiles are revealed by their owner**, and only then can someone be
  messaged.
- **Messages are ECIES-encrypted** (ephemeral X25519 → HKDF →
  XChaCha20-Poly1305) and signed with ML-DSA-65; an envelope is bound to
  its recipient so "they messaged you" cannot be forged.
- **HIV status never touches a public tile** — enforced by a test that
  scans serialized tile bytes for clinical terms, not by convention.
- **Favourites, notes and blocks never leave the device** except inside the
  passphrase-encrypted backup.

## What actually works

| Component | State |
| --- | --- |
| Freenet node on Android | **Works**, relaying live traffic |
| Contracts on mainnet | **Works** — presence, profile, inbox |
| Encrypted messaging | **Works** end to end over the network |
| Grid UI | **Works**, served *from* Freenet, self-updating |
| Per-install identity | **Works** — but the seed is in `localStorage`, not the Keystore |
| Onboarding, profile editor | **Missing** |
| Real GPS | **Missing** — location is hard-coded |
| Photos, albums | **Missing** |

**Do not use this as a dating app yet.** It cannot protect anyone until the
identity seed is behind the Android Keystore and real onboarding exists.

## Building

```bash
cargo test --workspace          # 121 tests, no network needed
cd web && dx serve              # the UI against an in-memory backend
cd android && gradle assembleDebug
```

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
