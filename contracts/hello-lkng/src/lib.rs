//! hello-lkng — LKNG's pipeline-proving contract and Gate 1 probe object.
//!
//! A deliberately tiny **grow-only set of strings**: the simplest state that
//! is a genuine commutative monoid (merge = set union, order-independent),
//! exercising the full `ContractInterface` the way the real LKNG contracts
//! will. Shape copied from Delta's site contract; two ecosystem lessons are
//! load-bearing here:
//!
//! * **Empty deltas must be ZERO BYTES** (Delta #5072): core's convergence
//!   check tests byte-emptiness, and a CBOR-encoded empty struct is ~never
//!   empty. `get_state_delta` returns `Vec::new()` when nothing is missing.
//! * **Caps are enforced by bounding what may be ADDED, not by rejecting
//!   merged state** (Raven's index shard): a transiently over-bound merge is
//!   normal; refusing it would break convergence.

use std::collections::BTreeSet;

use ciborium::{de::from_reader, ser::into_writer};
use freenet_stdlib::prelude::*;
use serde::{Deserialize, Serialize};

/// Max bytes per entry. Oversized entries are invalid anywhere they appear.
pub const MAX_ENTRY_BYTES: usize = 256;
/// Soft cap used to bound a single update's contribution.
pub const MAX_ENTRIES_PER_UPDATE: usize = 64;

/// Grow-only set. Merge = union; commutative, associative, idempotent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HelloState {
    pub entries: BTreeSet<String>,
}

/// Summary = the full entry set (fine at this size; real contracts summarize
/// by hash). A peer's delta is exactly the entries it's missing.
pub type HelloSummary = BTreeSet<String>;

/// Delta = entries to add.
pub type HelloDelta = Vec<String>;

fn entry_ok(e: &str) -> bool {
    !e.is_empty() && e.len() <= MAX_ENTRY_BYTES
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8], what: &str) -> Result<T, ContractError> {
    from_reader(bytes).map_err(|e| ContractError::Deser(format!("{what}: {e}")))
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ContractError> {
    let mut buf = Vec::new();
    into_writer(value, &mut buf).map_err(|e| ContractError::Deser(e.to_string()))?;
    Ok(buf)
}

impl HelloState {
    pub fn validate(&self) -> Result<(), String> {
        for e in &self.entries {
            if !entry_ok(e) {
                return Err(format!("entry invalid (len {} > {MAX_ENTRY_BYTES} or empty)", e.len()));
            }
        }
        Ok(())
    }

    /// Union-merge `other` into `self`, skipping invalid entries.
    pub fn merge(&mut self, other: &HelloState) {
        self.entries
            .extend(other.entries.iter().filter(|e| entry_ok(e)).cloned());
    }

    /// Apply a delta: add valid entries, bounded per update.
    pub fn apply_delta(&mut self, delta: &HelloDelta) {
        self.entries.extend(
            delta
                .iter()
                .filter(|e| entry_ok(e))
                .take(MAX_ENTRIES_PER_UPDATE)
                .cloned(),
        );
    }

    /// Entries the peer (per its summary) is missing. `None` when nothing —
    /// the caller MUST map that to zero bytes, not an encoded empty vec.
    pub fn delta_for(&self, peer_has: &HelloSummary) -> Option<HelloDelta> {
        let missing: Vec<String> = self.entries.difference(peer_has).cloned().collect();
        if missing.is_empty() {
            None
        } else {
            Some(missing)
        }
    }
}

struct Contract;

#[contract]
impl ContractInterface for Contract {
    fn validate_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
        _related: RelatedContracts<'static>,
    ) -> Result<ValidateResult, ContractError> {
        if state.as_ref().is_empty() {
            return Ok(ValidateResult::Valid);
        }
        let hello: HelloState = decode(state.as_ref(), "state")?;
        hello
            .validate()
            .map(|_| ValidateResult::Valid)
            .map_err(|reason| ContractError::InvalidUpdateWithInfo { reason })
    }

    fn update_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
        data: Vec<UpdateData<'static>>,
    ) -> Result<UpdateModification<'static>, ContractError> {
        let mut hello: HelloState = if state.as_ref().is_empty() {
            HelloState::default()
        } else {
            decode(state.as_ref(), "state")?
        };

        for update in data {
            match update {
                UpdateData::State(new_state) => {
                    let other: HelloState = decode(new_state.as_ref(), "incoming state")?;
                    hello.merge(&other);
                }
                UpdateData::Delta(d) => {
                    if d.as_ref().is_empty() {
                        continue;
                    }
                    let delta: HelloDelta = decode(d.as_ref(), "delta")?;
                    hello.apply_delta(&delta);
                }
                _ => {}
            }
        }

        Ok(UpdateModification::valid(State::from(encode(&hello)?)))
    }

    fn summarize_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
    ) -> Result<StateSummary<'static>, ContractError> {
        let summary: HelloSummary = if state.as_ref().is_empty() {
            HelloSummary::default()
        } else {
            let hello: HelloState = decode(state.as_ref(), "state")?;
            hello.entries
        };
        Ok(StateSummary::from(encode(&summary)?))
    }

    fn get_state_delta(
        _parameters: Parameters<'static>,
        state: State<'static>,
        summary: StateSummary<'static>,
    ) -> Result<StateDelta<'static>, ContractError> {
        let hello: HelloState = if state.as_ref().is_empty() {
            HelloState::default()
        } else {
            decode(state.as_ref(), "state")?
        };
        let peer_has: HelloSummary = if summary.as_ref().is_empty() {
            HelloSummary::default()
        } else {
            decode(summary.as_ref(), "summary")?
        };

        // Delta lesson #5072: nothing-to-send MUST be zero bytes.
        let buf = match hello.delta_for(&peer_has) {
            Some(delta) => encode(&delta)?,
            None => Vec::new(),
        };
        Ok(StateDelta::from(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> HelloState {
        HelloState {
            entries: items.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn merge_is_order_independent() {
        let (a, b, c) = (s(&["x"]), s(&["y"]), s(&["z", "x"]));
        let mut ab = a.clone();
        ab.merge(&b);
        ab.merge(&c);
        let mut cb = c.clone();
        cb.merge(&b);
        cb.merge(&a);
        assert_eq!(ab, cb, "union must not care about order");
    }

    #[test]
    fn merge_is_idempotent() {
        let mut a = s(&["x", "y"]);
        let snapshot = a.clone();
        a.merge(&snapshot.clone());
        assert_eq!(a, snapshot);
    }

    #[test]
    fn oversized_entries_rejected_everywhere() {
        let big = "b".repeat(MAX_ENTRY_BYTES + 1);
        let bad = s(&[big.as_str()]);
        assert!(bad.validate().is_err());
        let mut clean = s(&["ok"]);
        clean.merge(&bad);
        assert_eq!(clean, s(&["ok"]), "merge must drop invalid entries");
        let mut clean2 = s(&["ok"]);
        clean2.apply_delta(&vec![big]);
        assert_eq!(clean2, s(&["ok"]));
    }

    #[test]
    fn empty_delta_is_none_never_encoded() {
        let a = s(&["x"]);
        let peer: HelloSummary = a.entries.clone();
        assert!(a.delta_for(&peer).is_none(), "identical peers -> None -> zero bytes");
    }

    #[test]
    fn delta_carries_exactly_whats_missing() {
        let a = s(&["x", "y", "z"]);
        let peer: HelloSummary = s(&["y"]).entries;
        let d = a.delta_for(&peer).unwrap();
        assert_eq!(d, vec!["x".to_string(), "z".to_string()]);
    }

    #[test]
    fn cbor_roundtrip() {
        let a = s(&["hello", "lkng"]);
        let bytes = encode(&a).unwrap();
        let back: HelloState = decode(&bytes, "test").unwrap();
        assert_eq!(a, back);
    }
}
