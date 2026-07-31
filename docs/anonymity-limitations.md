# What LKNG hides — and what it does not

> This document ships with the app and must stay in plain language. It is a
> commitment, not marketing. If a claim here stops being true, fixing the
> claim is a release blocker.

## What LKNG protects

- **Your exact location.** Your phone converts your position to a coarse
  area (~5 km cell) *before* anything leaves the device, with a stable
  random offset applied first. No distance numbers exist anywhere — the
  trilateration attacks that plagued centralized apps have nothing to
  measure.
- **Your messages.** End-to-end encrypted between you and the person you
  matched with. The machines that carry them see ciphertext.
- **Your data, from a company.** There is no LKNG server, no account
  database, no analytics, no ad SDK. Nobody can sell what nobody holds.
- **Who reported whom.** Feedback and reports are cryptographically
  unlinkable to the person who filed them.

## What LKNG does NOT protect

- **Your face is public if you use a grid photo.** Anyone in your area can
  see your tile, and a determined scraper can record it. Face-matching can
  link the same photo across time even though your identity keys rotate.
  Use a photo you accept being public, or none.
- **Your IP address is visible to a small set of network neighbours** (not
  to people browsing your profile). This is comparable to — not worse
  than — a centralized app's server seeing your IP, but the parties are
  strangers rather than one company.
- **Published data cannot be recalled.** Content is replicated to other
  machines. "Delete" removes it from view and stops distribution; it
  cannot reach into every copy. Post accordingly.
- **Location claims by others may be false.** Nothing can force a stranger's
  device to tell the truth about where it is. Treat "nearby" as a claim,
  not a fact — as on every app in this category.
- **First-session gateway.** Until your on-phone node finishes joining the
  network, the app may use an official gateway, which (like any website)
  sees your IP and requests during that window. You can disable this in
  settings at the cost of a slower first start.

## The one-sentence version

LKNG removes the company from the middle and blunts location precision;
it does not make a public photo grid private, and it cannot un-publish
what has been published.
