# LKNG

A Grindr-class geosocial app with no company in the middle: free, open source,
no ads, no data harvesting. Profiles, coarse nearby discovery, and end-to-end
encrypted messaging carried by the modern [Freenet](https://freenet.org)
network, with the node running on the user's own phone.

**Status: pre-alpha — Phase 0 (feasibility gates) in progress.**

## Why

The mainstream apps' free tiers are deliberately degraded and ad-saturated;
their paid tiers cost hundreds of dollars a year and partly sell surveillance
back to the user; their central databases have produced documented real-world
harm. The paywall and the harvesting are one business model seen from two
sides. LKNG removes both at once by having no company in the middle.

## Honesty

LKNG is meaningfully better than centralized alternatives on location
precision (coarse cells, never distances) and on the absence of a central
database. It is **not** an anonymity system: peers see IPs within the ring
topology, a public photo grid is inherently scrapeable, and published data
cannot be universally deleted. `docs/anonymity-limitations.md` states exactly
what LKNG hides and what it does not, in plain language, and is reachable
from inside the app.

## Layout

```text
docs/        PLAN.md (the full plan) · threat model · privacy model
rust/        lkng-transport (backend-agnostic trait) · lkng-transport-mock
contracts/   profile · presence · inbox · conversation · media · reputation · moderation
delegates/   identity · location · encryption · aft-tokens · local-blocking
web/         frontend
tests/       contract-tests · integration-tests · simulation · scraping-sim
scripts/     build and Gate-1 experiment scripts
```

## Building

```bash
cargo test --workspace       # transport trait + mock, no network needed
```

Requires stable Rust. Contracts additionally need the
`wasm32-unknown-unknown` target and [`fdev`](https://freenet.org/dev/).

## License

AGPL-3.0-only. The bundled Freenet node is unmodified `freenet-core`
(AGPL, upstream); per upstream's LICENSE.md, apps communicating with it over
WebSocket are not derivative works — LKNG is AGPL by choice, because software
asking for this much trust should be auditable and stay that way.
