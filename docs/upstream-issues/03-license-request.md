# Could `harvest` / `ghostkeys` add a LICENSE file?

Both repos currently have no LICENSE, while the rest of the ecosystem
(`river`, `delta`, `mail`, `raven`, `freenet-scaffold`, `atlas`,
`freenet-stdlib`) is LGPL-3.0.

We're building an app on Freenet and both repos contain patterns we'd like
to build on rather than reinvent:

- **harvest** — the mutual blind-signature feedback token flow (RFC 9474)
  and the append-only `ReputationStateV1` contract. Accountability without
  identity is exactly the problem we have, and this is the best treatment
  of it we've seen.
- **ghostkeys** — the delegate authorization model: runtime-attested
  `SignatureRequestor`, least-privilege `GhostkeyScope`, and the
  `ScopedPayload` discipline ("the raw payload is never signed alone").
  Reading that comment led us to find and fix a signature-replay bug in
  our own code, so it's already earned its keep as documentation.

Without a license we can study the designs but can't reuse any code. If
LGPL-3.0 matches the rest of the ecosystem's intent, that would be ideal;
anything explicit is better than the current ambiguity.

Thanks for building all of this in the open.
