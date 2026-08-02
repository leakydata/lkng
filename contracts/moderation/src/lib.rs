//! moderation-contract — one instance per feed, anyone may append.
//!
//! Deliberately permissionless: a feed nobody but an appointed moderator
//! could write to would need an appointed moderator, and there isn't one.
//! What the contract enforces instead is that every report is **signed for
//! this feed** — so reports cannot be forged in someone else's name, and
//! cannot be lifted out of a permissive feed and replayed into a strict one.
//!
//! Whether a report *means* anything is not the contract's business. That is
//! decided by clients, from the feeds they chose to subscribe to.

use ciborium::{de::from_reader, ser::into_writer};
use freenet_stdlib::prelude::*;
use lkng_moderation::{FeedParams, FeedState, Report};
use serde::{Deserialize, Serialize};

pub use lkng_moderation::FeedParams as Params;

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
        let params: FeedParams = decode(parameters.as_ref(), "params")?;
        if state.as_ref().is_empty() {
            return Ok(ValidateResult::Valid);
        }
        let feed: FeedState = decode(state.as_ref(), "state")?;

        // Per-record checks only. The cap is NOT checked here: a merged
        // state that is briefly over it is normal on a network where updates
        // arrive in any order, and rejecting one would leave peers
        // permanently disagreeing.
        for r in feed.reports.values() {
            lkng_moderation::verify::verify_report(r, &params).map_err(|e| {
                ContractError::InvalidUpdateWithInfo {
                    reason: format!("report failed verification: {e}"),
                }
            })?;
        }
        Ok(ValidateResult::Valid)
    }

    fn update_state(
        parameters: Parameters<'static>,
        state: State<'static>,
        data: Vec<UpdateData<'static>>,
    ) -> Result<UpdateModification<'static>, ContractError> {
        let params: FeedParams = decode(parameters.as_ref(), "params")?;
        let mut feed: FeedState = if state.as_ref().is_empty() {
            FeedState::default()
        } else {
            decode(state.as_ref(), "state")?
        };

        for update in data {
            let incoming: Vec<Report> = match update {
                UpdateData::State(s) => {
                    decode::<FeedState>(s.as_ref(), "incoming")?
                        .reports
                        .into_values()
                        .collect()
                }
                UpdateData::Delta(d) => {
                    if d.as_ref().is_empty() {
                        continue;
                    }
                    // A list of reports, matching how clients build a delta.
                    decode(d.as_ref(), "delta")?
                }
                _ => continue,
            };
            for r in incoming {
                // Verify on the way in. An unverified report reaching state
                // would be rejected wholesale by the next `validate_state`,
                // which turns one bad record into a feed nobody can update.
                if lkng_moderation::verify::verify_report(&r, &params).is_ok() {
                    feed.insert(r);
                }
            }
        }

        // Trim after merging, never before, and never as a rejection.
        feed.trim();
        Ok(UpdateModification::valid(State::from(encode(&feed)?)))
    }

    fn summarize_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
    ) -> Result<StateSummary<'static>, ContractError> {
        let ids: Vec<[u8; 32]> = if state.as_ref().is_empty() {
            Vec::new()
        } else {
            decode::<FeedState>(state.as_ref(), "state")?
                .reports
                .keys()
                .copied()
                .collect()
        };
        Ok(StateSummary::from(encode(&ids)?))
    }

    fn get_state_delta(
        _parameters: Parameters<'static>,
        state: State<'static>,
        summary: StateSummary<'static>,
    ) -> Result<StateDelta<'static>, ContractError> {
        let feed: FeedState = if state.as_ref().is_empty() {
            FeedState::default()
        } else {
            decode(state.as_ref(), "state")?
        };
        let theirs: Vec<[u8; 32]> = if summary.as_ref().is_empty() {
            Vec::new()
        } else {
            decode(summary.as_ref(), "summary")?
        };
        let have: std::collections::BTreeSet<[u8; 32]> = theirs.into_iter().collect();
        let missing: Vec<Report> = feed
            .reports
            .iter()
            .filter(|(id, _)| !have.contains(*id))
            .map(|(_, r)| r.clone())
            .collect();

        // Zero bytes when synced (#5072). An encoded empty vec is NOT the
        // same thing: it is a non-empty delta that never converges.
        let buf = if missing.is_empty() { Vec::new() } else { encode(&missing)? };
        Ok(StateDelta::from(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lkng_identity::Identity;
    use lkng_moderation::Reason;

    fn params() -> FeedParams {
        FeedParams { schema_v: 1, feed: "baseline".into() }
    }

    #[test]
    fn a_signed_report_verifies_and_a_tampered_one_does_not() {
        let reporter = Identity::from_seed([0x21; 32]).for_epoch(42);
        let mut r = Report {
            subject: [0xAB; 32],
            reason: Reason::Harassment.code(),
            note: "sent threats".into(),
            timestamp_ms: 1_785_633_000_000,
            verifying_key: None,
            sig: vec![],
        };
        reporter.sign_report(&mut r, &params()).unwrap();
        lkng_moderation::verify::verify_report(&r, &params()).unwrap();

        r.note = "sent flowers".into();
        assert!(
            lkng_moderation::verify::verify_report(&r, &params()).is_err(),
            "editing a report must invalidate it"
        );
    }

    /// A report signed for one feed must not verify in another.
    ///
    /// Otherwise anyone could harvest reports from a permissive feed and
    /// replay them into a strict one, manufacturing consensus out of
    /// statements nobody made there.
    #[test]
    fn a_report_cannot_be_replayed_into_another_feed() {
        let reporter = Identity::from_seed([0x22; 32]).for_epoch(42);
        let mut r = Report {
            subject: [0xCD; 32],
            reason: Reason::Spam.code(),
            note: String::new(),
            timestamp_ms: 1,
            verifying_key: None,
            sig: vec![],
        };
        reporter.sign_report(&mut r, &params()).unwrap();
        let other = FeedParams { schema_v: 1, feed: "strict".into() };
        assert!(lkng_moderation::verify::verify_report(&r, &other).is_err());
    }
}
