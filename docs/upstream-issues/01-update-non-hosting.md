# UPDATE still fails with "missing contract" from a non-hosting node on 0.2.116 (regression of #4066 / #4071)

## Summary

On `freenet 0.2.116` (release binary, network mode, connected to public
gateways), a client-initiated `Update` for a contract the local node does
**not** host fails with:

```
UPDATE failed: missing contract: <key>
```

`GET` for the same key succeeds from the same node in under a second and
returns full state. `GET --return-code` also succeeds but does **not**
populate the local store, so the subsequent `UPDATE` fails identically.

This is the behaviour described in #4066 ("Get-with-subscribe +
return_contract_code doesn't reliably populate caller's local store") and
#4071 ("UPDATE from non-hosting originator returns 'missing contract
parameters' instead of self-healing"), both closed 2026-05-09. It
reproduces on a single node against the live network.

## Reproduction

Contract: a small anyone-writes grow-set (state is a CBOR map of
content-addressed records), parameters are a CBOR struct. Nothing exotic —
no related contracts, ~5.5 KB state, ~5.5 KB delta.

```bash
# 1. publish, subscribing on the way in
fdev publish --code presence_cell.wasm --parameters cell_params.bin \
             --subscribe contract --state initial_state.bin
# -> "Contract published successfully, response_key: 6TUPTa5dCuC..."

# 2. immediately update (no delay, no restart)
fdev execute update 6TUPTa5dCuC... delta.bin --timeout 120
# -> Error: UPDATE failed: missing contract: 6TUPTa5dCuC...
```

Variations that make no difference:

- publishing **with** and **without** `--subscribe`
- an explicit `fdev execute subscribe` first (the subscription *does*
  appear in `fdev diagnostics` under "Subscriptions")
- `fdev execute get --return-code` first (returns 5582 bytes successfully)
- `--as-state` instead of a delta
- issuing the UPDATE from a second node on the same host (separate
  config/data dirs and ports)
- waiting minutes vs. issuing immediately after publish

## The telling diagnostic

Right after a successful publish and a successful 5582-byte GET:

```
📋 Subscriptions:
| E4SWQ7188dkvuohrfTfjohn5YLQyoHC4j6utj85j2xgj | 799000002 |

📄 Contract States:
| 6TUPTa5dCuCLBPjZGxuzW7BtYMDMD6BTChY13hcBE9sp | 0           | None    |

📊 System Metrics:
  Active connections: 54
  Hosting contracts: 214
```

The node holds 214 contracts, is well connected, is subscribed — and
reports **0 / None** for the contract it just published and can read.
`state_store.get(&key)` then returns `MissingContract`
(`crates/core/src/contract/executor/runtime/executor_impl.rs:697-707`),
so `drive_client_update` fails before forwarding.

## Why this is load-bearing

Per #4071's own analysis, the wire format
`UpdateMsg::RequestUpdate.value: WrappedState` carries a **post-merge full
state**, not a delta, so the originator must merge locally before
forwarding — which requires the contract's code and params locally.

The practical consequence is that **a node can only write to contracts it
happens to host**, and hosting follows ring location rather than user
interest. For any app with a shared multi-writer contract — a chat room, an
index, a per-area shard — a given client's ability to write to it is
effectively arbitrary.

We hit this building a geosocial app where discovery is a shared
per-area contract that many nearby users append to. Reads work perfectly;
writes work only from whichever node happens to host the shard.

## Environment

- `freenet 0.2.116 (1b3bf6cab018)`, `fdev 0.3.278`, stdlib `0.8.5`
- Linux x86_64, network mode, 54 active connections, node location 0.066278
- Reproduced from two independent nodes on the same host, and immediately
  after publish on the publishing node itself

## Happy to help

We have a minimal contract and scripted repro and can run any diagnostic
build or patch against the live network — just say what would be useful.
