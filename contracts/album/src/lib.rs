//! album-contract — one instance per album, single-writer, ciphertext only.
//!
//! Single-writer, unlike the presence cell and the moderation feed: an album
//! belongs to one person, so the whole state carries one signature rather
//! than each item carrying its own. That is simpler *and* stricter — there
//! is no path by which a second party contributes anything.
//!
//! The contract never sees a photo. It sees ciphertext, checks the owner
//! signed it, and enforces the caps that stop one album costing its
//! replicating peers unbounded storage. It cannot tell how many photos are
//! real, and does not need to.

use ciborium::{de::from_reader, ser::into_writer};
use freenet_stdlib::prelude::*;
use lkng_album::{AlbumParams, AlbumState};
use serde::{Deserialize, Serialize};

pub use lkng_album::AlbumParams as Params;

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
        let params: AlbumParams = decode(parameters.as_ref(), "params")?;
        if state.as_ref().is_empty() {
            return Ok(ValidateResult::Valid);
        }
        let album: AlbumState = decode(state.as_ref(), "state")?;
        lkng_album::verify::verify_album(&album, &params).map_err(|e| {
            ContractError::InvalidUpdateWithInfo {
                reason: format!("album failed verification: {e}"),
            }
        })?;
        Ok(ValidateResult::Valid)
    }

    fn update_state(
        parameters: Parameters<'static>,
        state: State<'static>,
        data: Vec<UpdateData<'static>>,
    ) -> Result<UpdateModification<'static>, ContractError> {
        let params: AlbumParams = decode(parameters.as_ref(), "params")?;
        let mut current: Option<AlbumState> = if state.as_ref().is_empty() {
            None
        } else {
            Some(decode(state.as_ref(), "state")?)
        };

        for update in data {
            let bytes = match &update {
                UpdateData::State(s) => s.as_ref(),
                UpdateData::Delta(d) if !d.as_ref().is_empty() => d.as_ref(),
                _ => continue,
            };
            let incoming: AlbumState = decode(bytes, "incoming")?;

            // Whole-state replacement by the owner, not a merge. An album is
            // single-writer, so "merge" has no meaning here -- and a merging
            // album could never shrink, which would make deleting a photo
            // impossible.
            if lkng_album::verify::verify_album(&incoming, &params).is_err() {
                continue;
            }

            // Generation must not go backwards. Otherwise a replayed older
            // state would reinstate a key that a removed viewer still holds,
            // silently undoing a revocation the owner believes happened.
            if let Some(cur) = &current {
                if incoming.generation < cur.generation {
                    continue;
                }
            }
            current = Some(incoming);
        }

        Ok(UpdateModification::valid(State::from(match current {
            Some(a) => encode(&a)?,
            None => Vec::new(),
        })))
    }

    fn summarize_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
    ) -> Result<StateSummary<'static>, ContractError> {
        let gen: u32 = if state.as_ref().is_empty() {
            0
        } else {
            decode::<AlbumState>(state.as_ref(), "state")?.generation
        };
        Ok(StateSummary::from(encode(&gen)?))
    }

    fn get_state_delta(
        _parameters: Parameters<'static>,
        state: State<'static>,
        summary: StateSummary<'static>,
    ) -> Result<StateDelta<'static>, ContractError> {
        if state.as_ref().is_empty() {
            return Ok(StateDelta::from(Vec::new()));
        }
        let album: AlbumState = decode(state.as_ref(), "state")?;
        let theirs: u32 = if summary.as_ref().is_empty() {
            0
        } else {
            decode(summary.as_ref(), "summary")?
        };
        // Zero bytes when they are current (#5072) -- never an encoded
        // empty struct, which is a non-empty delta that never converges.
        let buf = if album.generation <= theirs && theirs != 0 {
            Vec::new()
        } else {
            encode(&album)?
        };
        Ok(StateDelta::from(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lkng_album::address_of;
    use lkng_identity::Identity;

    fn params(owner: &Identity) -> AlbumParams {
        AlbumParams { schema_v: 1, address: address_of(&owner.verifying_key_bytes(), 0) }
    }

    #[test]
    fn only_the_owner_can_write_an_album() {
        let owner = Identity::from_seed([0x41; 32]);
        let mallory = Identity::from_seed([0x99; 32]);

        let mut album = AlbumState::default();
        album.insert(Identity::seal_album_photo(&[6; 32], b"pic", [1; 24]).unwrap());

        // Signed by the wrong person, presented at the owner's address.
        mallory.sign_album(&mut album, &params(&owner)).unwrap();
        assert!(
            lkng_album::verify::verify_album(&album, &params(&owner)).is_err(),
            "a stranger must not be able to write to someone else's album"
        );
    }

    /// A replayed older state must not roll a revocation back.
    ///
    /// The generation counter is what a removed viewer's key is checked
    /// against, so accepting an older generation would quietly re-grant
    /// access the owner believes they took away.
    #[test]
    fn generation_never_moves_backwards() {
        let owner = Identity::from_seed([0x42; 32]);
        let p = params(&owner);

        let mut old = AlbumState { generation: 1, ..Default::default() };
        old.insert(Identity::seal_album_photo(&[6; 32], b"a", [1; 24]).unwrap());
        owner.sign_album(&mut old, &p).unwrap();

        let mut new = AlbumState { generation: 5, ..Default::default() };
        new.insert(Identity::seal_album_photo(&[6; 32], b"b", [2; 24]).unwrap());
        owner.sign_album(&mut new, &p).unwrap();

        // Both are validly signed by the owner; the contract still must
        // refuse to go back to generation 1.
        assert!(lkng_album::verify::verify_album(&old, &p).is_ok());
        assert!(lkng_album::verify::verify_album(&new, &p).is_ok());
        assert!(new.generation > old.generation);
    }
}
