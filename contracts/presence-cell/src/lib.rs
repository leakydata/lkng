//! presence-cell — LKNG's discovery contract, one instance per
//! `(cell_id, epoch)`.
//!
//! Thin `ContractInterface` shell over [`lkng_presence::CellState`], which
//! owns all merge/cap/convergence logic (and its property tests). The
//! parameters pin WHICH cell+epoch this instance is — every distinct
//! parameter value is a distinct contract key, which is exactly how epoch
//! rollover replaces pruning.

use ciborium::{de::from_reader, ser::into_writer};
use freenet_stdlib::prelude::*;
use lkng_presence::{CellParams, CellState, PresenceRecord, RecordId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// `CellParams` lives in `lkng-presence` because record signatures must
// cover it (see `PresenceRecord::signing_payload`) — the parameters are a
// security input, not just a contract-shell detail. Re-exported so callers
// building parameters only need this crate.
pub use lkng_presence::CellParams as Params;

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8], what: &str) -> Result<T, ContractError> {
    from_reader(bytes).map_err(|e| ContractError::Deser(format!("{what}: {e}")))
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ContractError> {
    let mut buf = Vec::new();
    into_writer(value, &mut buf).map_err(|e| ContractError::Deser(e.to_string()))?;
    Ok(buf)
}

struct Contract;

#[contract]
impl ContractInterface for Contract {
    fn validate_state(
        parameters: Parameters<'static>,
        state: State<'static>,
        _related: RelatedContracts<'static>,
    ) -> Result<ValidateResult, ContractError> {
        // Parameters must decode (a mis-parameterized instance is dead on
        // arrival, better loudly than quietly).
        let _params: CellParams = decode(parameters.as_ref(), "params")?;
        if state.as_ref().is_empty() {
            return Ok(ValidateResult::Valid);
        }
        let cell: CellState = decode(state.as_ref(), "state")?;
        // Per-record invariants only. Deliberately NOT a MAX_RECORDS check —
        // transiently over-bound merged state is normal (Raven lesson #1).
        cell.validate()
            .map(|_| ValidateResult::Valid)
            .map_err(|e| ContractError::InvalidUpdateWithInfo { reason: e.to_string() })
    }

    fn update_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
        data: Vec<UpdateData<'static>>,
    ) -> Result<UpdateModification<'static>, ContractError> {
        let mut cell: CellState = if state.as_ref().is_empty() {
            CellState::default()
        } else {
            decode(state.as_ref(), "state")?
        };

        for update in data {
            match update {
                UpdateData::State(new_state) => {
                    let other: CellState = decode(new_state.as_ref(), "incoming state")?;
                    cell.merge(&other);
                }
                UpdateData::Delta(d) => {
                    if d.as_ref().is_empty() {
                        continue;
                    }
                    let delta: Vec<PresenceRecord> = decode(d.as_ref(), "delta")?;
                    cell.apply_records(delta);
                }
                _ => {}
            }
        }

        Ok(UpdateModification::valid(State::from(encode(&cell)?)))
    }

    fn summarize_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
    ) -> Result<StateSummary<'static>, ContractError> {
        let summary: BTreeSet<RecordId> = if state.as_ref().is_empty() {
            BTreeSet::new()
        } else {
            let cell: CellState = decode(state.as_ref(), "state")?;
            cell.summary()
        };
        Ok(StateSummary::from(encode(&summary)?))
    }

    fn get_state_delta(
        _parameters: Parameters<'static>,
        state: State<'static>,
        summary: StateSummary<'static>,
    ) -> Result<StateDelta<'static>, ContractError> {
        let cell: CellState = if state.as_ref().is_empty() {
            CellState::default()
        } else {
            decode(state.as_ref(), "state")?
        };
        let peer_has: BTreeSet<RecordId> = if summary.as_ref().is_empty() {
            BTreeSet::new()
        } else {
            decode(summary.as_ref(), "summary")?
        };

        // Delta #5072: nothing-to-send MUST be zero bytes, never an
        // encoded empty container.
        let buf = match cell.delta_for(&peer_has) {
            Some(delta) => encode(&delta)?,
            None => Vec::new(),
        };
        Ok(StateDelta::from(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(seed: u8, ts: u64) -> PresenceRecord {
        PresenceRecord {
            pseudonym: [seed; 32],
            headline: format!("t{seed}"),
            thumbnail: vec![seed; 32],
            timestamp_ms: ts,
            writer_cert: None,
            sig: vec![seed; 64],
        }
    }

    #[test]
    fn params_roundtrip() {
        let p = CellParams { schema_v: 1, cell_id: "9q8yy".into(), epoch: 42 };
        let bytes = encode(&p).unwrap();
        let back: CellParams = decode(&bytes, "t").unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn state_roundtrip_and_delta_zero_bytes_when_synced() {
        let mut cell = CellState::default();
        cell.insert(rec(1, 10));
        cell.insert(rec(2, 20));
        let bytes = encode(&cell).unwrap();
        let back: CellState = decode(&bytes, "t").unwrap();
        assert_eq!(cell, back);
        // Identical peers → None → contract emits zero bytes.
        assert!(cell.delta_for(&cell.summary()).is_none());
    }

    #[test]
    fn delta_carries_missing_records_only() {
        let mut a = CellState::default();
        a.insert(rec(1, 10));
        a.insert(rec(2, 20));
        let mut b = CellState::default();
        b.insert(rec(2, 20));
        let d = a.delta_for(&b.summary()).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].pseudonym, [1u8; 32]);
    }
}
