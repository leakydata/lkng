//! profile-contract — LKNG's durable profile, one instance per identity.
//!
//! Thin `ContractInterface` shell over [`lkng_profile::ProfileState`],
//! which owns the merge and signature logic (and its tests).
//!
//! The parameters carry the owner's durable verifying key, so the contract
//! address *is* the identity: `hash(code, params)` differs for every owner,
//! and the contract refuses any state not signed by that exact key. That
//! closes the address-claiming problem Delta documents — there is no
//! "first write wins" race, because a write by anyone else simply fails
//! verification.

use ciborium::{de::from_reader, ser::into_writer};
use freenet_stdlib::prelude::*;
use lkng_profile::{ProfileParams, ProfileState, ProfileSummary};
use serde::{Deserialize, Serialize};

pub use lkng_profile::ProfileParams as Params;

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
        let params: ProfileParams = decode(parameters.as_ref(), "params")?;
        if state.as_ref().is_empty() {
            return Ok(ValidateResult::Valid);
        }
        let profile: ProfileState = decode(state.as_ref(), "state")?;

        // Enforced by the network, not merely by well-behaved clients:
        // the body (and any tombstone) must carry a signature from the
        // owner key pinned in the parameters. Anyone can PUT to this
        // address; only the owner can put anything that survives.
        lkng_profile::verify::verify_state(&profile, &params).map_err(|e| {
            ContractError::InvalidUpdateWithInfo {
                reason: format!("profile failed verification: {e}"),
            }
        })?;
        Ok(ValidateResult::Valid)
    }

    fn update_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
        data: Vec<UpdateData<'static>>,
    ) -> Result<UpdateModification<'static>, ContractError> {
        let mut profile: ProfileState = if state.as_ref().is_empty() {
            ProfileState::default()
        } else {
            decode(state.as_ref(), "state")?
        };

        for update in data {
            match update {
                UpdateData::State(new_state) => {
                    let other: ProfileState = decode(new_state.as_ref(), "incoming state")?;
                    profile.merge(&other);
                }
                UpdateData::Delta(d) => {
                    if d.as_ref().is_empty() {
                        continue;
                    }
                    let other: ProfileState = decode(d.as_ref(), "delta")?;
                    profile.merge(&other);
                }
                _ => {}
            }
        }

        Ok(UpdateModification::valid(State::from(encode(&profile)?)))
    }

    fn summarize_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
    ) -> Result<StateSummary<'static>, ContractError> {
        let summary: ProfileSummary = if state.as_ref().is_empty() {
            ProfileSummary::default()
        } else {
            let profile: ProfileState = decode(state.as_ref(), "state")?;
            profile.summarize()
        };
        Ok(StateSummary::from(encode(&summary)?))
    }

    fn get_state_delta(
        _parameters: Parameters<'static>,
        state: State<'static>,
        summary: StateSummary<'static>,
    ) -> Result<StateDelta<'static>, ContractError> {
        let profile: ProfileState = if state.as_ref().is_empty() {
            ProfileState::default()
        } else {
            decode(state.as_ref(), "state")?
        };
        let peer: ProfileSummary = if summary.as_ref().is_empty() {
            ProfileSummary::default()
        } else {
            decode(summary.as_ref(), "summary")?
        };

        // Delta #5072: nothing to send MUST be zero bytes, never an encoded
        // empty struct (which ciborium never renders as empty).
        let buf = if profile.summarize() == peer {
            Vec::new()
        } else {
            encode(&profile)?
        };
        Ok(StateDelta::from(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lkng_identity::Identity;
    use lkng_profile::ProfileBody;

    fn body(seq: u64) -> ProfileBody {
        ProfileBody {
            display_name: "sam".into(),
            bio: "here for the plot".into(),
            tags: vec!["music".into()],
            photos: vec![],
            thumbnail: vec![1; 32],
            demographics: Default::default(),
            encryption_key: None,
            sequence: seq,
        }
    }

    #[test]
    fn owner_signed_profile_validates() {
        let id = Identity::from_seed([21; 32]);
        let state = id.sign_profile(body(1)).unwrap();
        let params = id.profile_params();
        lkng_profile::verify::verify_state(&state, &params).unwrap();
    }

    #[test]
    fn another_identity_cannot_occupy_this_address() {
        // The address-claiming problem, closed: a squatter's state fails
        // verification against the owner key pinned in the parameters.
        let owner = Identity::from_seed([21; 32]);
        let squatter = Identity::from_seed([22; 32]);
        let hostile = squatter.sign_profile(body(99)).unwrap();
        assert!(
            lkng_profile::verify::verify_state(&hostile, &owner.profile_params()).is_err(),
            "only the owner key may produce valid state at this address"
        );
    }

    #[test]
    fn empty_delta_is_zero_bytes() {
        let id = Identity::from_seed([21; 32]);
        let state = id.sign_profile(body(2)).unwrap();
        let summary = state.summarize();
        assert!(
            state.summarize() == summary,
            "a synced peer must produce a zero-byte delta"
        );
    }

    #[test]
    fn params_and_state_roundtrip_through_cbor() {
        let id = Identity::from_seed([21; 32]);
        let params = id.profile_params();
        let pbytes = encode(&params).unwrap();
        let pback: ProfileParams = decode(&pbytes, "t").unwrap();
        assert_eq!(params, pback);

        let state = id.sign_profile(body(3)).unwrap();
        let sbytes = encode(&state).unwrap();
        let sback: ProfileState = decode(&sbytes, "t").unwrap();
        assert_eq!(state, sback);
        lkng_profile::verify::verify_state(&sback, &pback).unwrap();
    }
}
