# `fdev execute update` cannot work on contracts the node doesn't host, because each invocation is a new session

## Summary

`fdev execute update <key> <delta>` fails with `UPDATE failed: missing
contract: <key>` for any contract the local node does not already host —
even immediately after `fdev publish --subscribe` of that same contract,
and even though `fdev execute get` on the same key returns full state in
under a second.

The same operation succeeds reliably through the client API when PUT and
UPDATE share **one WebSocket session**. So this is not a core defect; it is
a gap in `fdev`'s process model plus an error message that points at the
wrong thing.

## Cause

An UPDATE is applied locally before forwarding (the wire format carries a
post-merge full state, not a raw delta — per the analysis in #4071), so the
originator needs the contract's code and parameters in its own store.
River seeds exactly that with a `ContractRequest::Put` carrying code +
params + state and `subscribe: true`, over a long-lived `WebApi` that then
serves every subsequent request.

`fdev` opens a **fresh WebSocket per invocation**, so `fdev publish` and
the following `fdev execute update` are different sessions and the second
one has nothing seeded.

## Reproduction

Fails:

```bash
fdev publish --code c.wasm --parameters p.bin --subscribe contract --state s.bin
fdev execute update <key> delta.bin        # missing contract
```

Succeeds — same node, same contract, same bytes, one session:

```
connected (one session for everything)
seed PUT ok: 3fWWKKoRC9Y2JTtLcJ8cwT8mT5Gu7HGRsDGt1NtWhkSf
state before update: 5582 bytes
UPDATE ACCEPTED — state now 11157 bytes (was 5582)
```

(~90 lines of `freenet-stdlib` client API; happy to share the harness.)

## Suggestions

1. **Error message.** `missing contract: <key>` reads as "this contract
   doesn't exist", which sends you hunting eviction, propagation and
   subscription. Something like *"contract not in local store — PUT it
   with code and parameters on this session before updating"* would have
   saved a day.
2. **`fdev execute update --seed <code> --parameters <params>`**, doing the
   seed PUT and the UPDATE on one connection, so the CLI can express the
   only pattern that works.
3. **Docs.** The seed-PUT requirement is discoverable today only by reading
   River's `room_synchronizer.rs`. A line in the contract docs saying "to
   update a contract your node does not host, first PUT code + params +
   state on the same session" would make multi-writer contracts approachable.

## Environment

`freenet 0.2.116 (1b3bf6cab018)`, `fdev 0.3.278`, stdlib `0.8.5`, Linux
x86_64, network mode, 54 connections. Related: #4066, #4071.
