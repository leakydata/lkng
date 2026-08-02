# Store listing and policy compliance

Working notes for publishing LKNG to Google Play, F-Droid and direct APK.
Three channels, so no single delisting ends distribution — which for this
user base is itself a safety property.

## Data Safety form (Google Play)

The unusual part of this listing: almost every answer is "no", and each
answer is structurally true rather than a policy promise.

| Question | Answer | Why |
| --- | --- | --- |
| Does your app collect or share user data? | **No** | There is no developer server. Nothing is transmitted to any party operated by this project. |
| Data encrypted in transit? | Yes | All peer traffic is encrypted; messages are additionally end-to-end encrypted. |
| Can users request data deletion? | Yes, locally | Uninstalling deletes everything held on device. Data already replicated to other peers cannot be recalled — disclosed in the listing and in the policy. |
| Location collected? | **No** (by the developer) | Coarse location is used on-device to derive a ~5 km cell. Neither the exact position nor the cell is sent to any developer-operated service. |
| Photos collected? | **No** (by the developer) | A thumbnail is published to a peer-to-peer network at the user's instruction, not collected by us. |

The distinction Play cares about is *who receives* the data. "Published by
the user to a decentralised network" is not "collected by the developer",
and the listing must explain that plainly rather than leaning on the
technicality.

## Required disclosures

- **Foreground service.** The app runs a Freenet node in a foreground
  service with a persistent notification and a one-tap Stop. Declared as
  `dataSync`. The notification is the honest choice as well as the required
  one: a phone quietly joined to a P2P network is not something to hide.
- **User-generated content.** Profiles, photos and messages. Requires
  in-app reporting, blocking, and a stated moderation process — all
  implemented, and implemented for ethical reasons rather than to satisfy
  this checklist.
- **18+ content rating.** Dating app with adult UGC.
- **Privacy policy URL.** [`docs/privacy-policy.md`](privacy-policy.md),
  published at a stable URL before submission.

## Age assurance — the unresolved part

This is a genuine legal exposure with no clean decentralised answer, and it
should be treated as an open risk rather than a solved item.

LKNG uses a self-declared date of birth (`lkng_app::check_age`, tested at
the boundary). That satisfies the letter of the store rating requirement.
It does **not** satisfy the UK Online Safety Act's "highly effective age
assurance" duty, nor several US state laws now requiring verification for
adult-oriented services.

Every available method of real verification requires an identity provider
holding documents — the exact centralised party this project exists
without, and one whose breach would be far worse for these users than for
most. The options, none good:

1. **Ship with self-declaration and geoblock** the jurisdictions with
   verification duties. Achievable; unpleasant, and unreliable given a
   user-set location.
2. **Integrate a third-party age-verification provider** where legally
   required. Undermines the central claim, and creates a database of people
   who use a gay dating app — the precise harm the design exists to prevent.
3. **Ship to F-Droid and direct APK only** in those jurisdictions, treating
   Play as the channel that carries the compliance burden.

Current position: option 1 for the first release, documented rather than
quietly ignored, and revisited before any jurisdiction-specific launch.
Legal advice is needed here and has not been obtained.

## F-Droid

- Reproducible builds are the point of this channel, so the bundled node
  binary must be built from pinned upstream source in the recipe rather
  than committed as a blob.
- `freenet-core` pins Rust 1.94.0; the recipe must pin the same.
- AGPL-3.0-only, all dependencies free — no anti-features to declare.

## Pre-submission checklist

- [ ] Signed release APK with a key held offline
- [ ] Privacy policy at a stable public URL
- [x] Age gate wired into first run
- [x] Reporting flow reachable from every profile
- [ ] Moderation feed subscribed by default
- [x] Account deletion: local wipe plus tombstone, with the honest caveat
- [ ] Screenshots that do not depict real people
- [ ] Battery and Doze behaviour measured over hours, not minutes
- [ ] Tested on more than one device
