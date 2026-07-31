//! inbox-contract — message requests, one instance per recipient.
//!
//! Anyone may append an envelope; only the recipient's key can produce a
//! valid processed-set. Both halves are enforced here rather than trusted
//! to clients: an inbox is the one contract strangers write to by design,
//! so it is also the one most worth flooding.

use ciborium::{de::from_reader, ser::into_writer};
use freenet_stdlib::prelude::*;
use lkng_inbox::{InboxParams, InboxState, InboxSummary};
use serde::{Deserialize, Serialize};

pub use lkng_inbox::InboxParams as Params;

fn decode<T: for<'de> Deserialize<'de>>(b: &[u8], what: &str) -> Result<T, ContractError> {
    from_reader(b).map_err(|e| ContractError::Deser(format!("{what}: {e}")))
}

fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>, ContractError> {
    let mut buf = Vec::new();
    into_writer(v, &mut buf).map_err(|e| ContractError::Deser(e.to_string()))?;
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
        let params: InboxParams = decode(parameters.as_ref(), "params")?;
        if state.as_ref().is_empty() {
            return Ok(ValidateResult::Valid);
        }
        let inbox: InboxState = decode(state.as_ref(), "state")?;
        // Every envelope must be signed for THIS inbox, and any
        // processed-set must be signed by the recipient. Without the
        // second check anyone could mark a stranger's messages read and
        // hide them.
        lkng_inbox::verify::verify_state(&inbox, &params).map_err(|e| {
            ContractError::InvalidUpdateWithInfo {
                reason: format!("inbox failed verification: {e}"),
            }
        })?;
        Ok(ValidateResult::Valid)
    }

    fn update_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
        data: Vec<UpdateData<'static>>,
    ) -> Result<UpdateModification<'static>, ContractError> {
        let mut inbox: InboxState = if state.as_ref().is_empty() {
            InboxState::default()
        } else {
            decode(state.as_ref(), "state")?
        };
        for update in data {
            match update {
                UpdateData::State(s) => inbox.merge(&decode(s.as_ref(), "incoming")?),
                UpdateData::Delta(d) => {
                    if d.as_ref().is_empty() {
                        continue;
                    }
                    inbox.merge(&decode(d.as_ref(), "delta")?);
                }
                _ => {}
            }
        }
        Ok(UpdateModification::valid(State::from(encode(&inbox)?)))
    }

    fn summarize_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
    ) -> Result<StateSummary<'static>, ContractError> {
        let summary: InboxSummary = if state.as_ref().is_empty() {
            InboxSummary::default()
        } else {
            decode::<InboxState>(state.as_ref(), "state")?.summary()
        };
        Ok(StateSummary::from(encode(&summary)?))
    }

    fn get_state_delta(
        _parameters: Parameters<'static>,
        state: State<'static>,
        summary: StateSummary<'static>,
    ) -> Result<StateDelta<'static>, ContractError> {
        let inbox: InboxState = if state.as_ref().is_empty() {
            InboxState::default()
        } else {
            decode(state.as_ref(), "state")?
        };
        let peer: InboxSummary = if summary.as_ref().is_empty() {
            InboxSummary::default()
        } else {
            decode(summary.as_ref(), "summary")?
        };
        // Zero bytes when synced (#5072), never an encoded empty struct.
        let buf = match inbox.delta_for(&peer) {
            Some(d) => encode(&d)?,
            None => Vec::new(),
        };
        Ok(StateDelta::from(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lkng_identity::Identity;

    #[test]
    fn stranger_can_write_owner_can_read() {
        let bob = Identity::from_seed([0xB0; 32]);
        let alice = Identity::from_seed([0xA1; 32]);
        let env = alice
            .seal_message(
                &bob.encryption_public_key(),
                &bob.verifying_key_bytes(),
                20670,
                b"hello",
                1,
            )
            .unwrap();
        let mut state = InboxState::default();
        state.insert(env);
        lkng_inbox::verify::verify_state(&state, &bob.inbox_params()).unwrap();
        assert_eq!(bob.open_message(state.pending()[0]).unwrap(), b"hello");
    }

    #[test]
    fn forged_processed_set_rejected_by_the_contract() {
        let bob = Identity::from_seed([0xB0; 32]);
        let mallory = Identity::from_seed([0x77; 32]);
        let mut state = InboxState::default();
        state.processed.ids.insert([1u8; 32]);
        mallory.sign_processed(&mut state).unwrap(); // wrong signer
        assert!(lkng_inbox::verify::verify_state(&state, &bob.inbox_params()).is_err());
    }
}
