//! Private albums: photos shared with named people, never published.
//!
//! # The constraint everything here follows from
//!
//! **A private photo on a public network cannot be un-shared.** Freenet
//! replicates; there is no server to delete from and no way to make a peer
//! forget bytes it already holds. So the design cannot rely on withdrawing
//! access later — it has to be that the bytes were never readable in the
//! first place.
//!
//! Hence: the album contract holds **ciphertext only**. Anyone may fetch it,
//! and it is meaningless to them. What is shared is not the photo but the
//! key, sealed individually to each person the owner names.
//!
//! # Why the key travels separately from the photos
//!
//! An album is potentially megabytes; an inbox envelope is capped at 4 KiB
//! because every unsolicited message anyone can send you is a cost you did
//! not consent to. So the photo bytes live in a contract addressed by the
//! owner, and a **grant** — a 32-byte key plus the album's address — travels
//! through the inbox, which is exactly the size that channel is built for.
//!
//! This split has a second benefit that matters more than the first: adding
//! a viewer costs one small message, not a re-upload. Sharing an album with
//! ten people uploads the photos once.
//!
//! # Revocation is prospective, and the UI must say so
//!
//! Removing someone from an album stops them decrypting **future** photos,
//! because those are encrypted under a new key. It does nothing about photos
//! they could already read: they may have downloaded the ciphertext, they
//! hold the old key, and both of those are now facts about their device.
//!
//! Every app in this category quietly implies otherwise. Saying it plainly
//! is the difference between a user making an informed decision about a
//! photo and making one they think they can take back.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Domain tag for album signatures. Wire format.
pub const SIG_DOMAIN: &str = "lkng/album/v1";

/// Domain tag for the grant payload carried through an inbox envelope.
pub const GRANT_DOMAIN: &[u8] = b"lkng/album-grant/v1";

/// Cap on a single album photo, in bytes.
///
/// Larger than a grid thumbnail (16 KiB) because an album photo is fetched
/// deliberately by a handful of people rather than pushed to everyone in a
/// cell. Still capped: the contract is replicated by peers who did not ask
/// for it, and freenet-git's measurements put the practical per-contract
/// ceiling in the low single-digit megabytes.
pub const MAX_PHOTO_BYTES: usize = 256 * 1024;

/// Cap on photos per album.
pub const MAX_PHOTOS: usize = 24;

/// A symmetric album key.
pub const KEY_BYTES: usize = 32;

pub const MAX_SIG_BYTES: usize = 4096;
pub const ML_DSA_65_VK_BYTES: usize = 1952;
pub const ADDRESS_BYTES: usize = 16;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AlbumError {
    #[error("photo exceeds {MAX_PHOTO_BYTES} bytes")]
    PhotoTooLarge,
    #[error("album holds more than {MAX_PHOTOS} photos")]
    TooManyPhotos,
    #[error("signature is empty or exceeds {MAX_SIG_BYTES} bytes")]
    MalformedSignature,
    #[error("owner key is not a valid ML-DSA-65 key")]
    BadOwnerKey,
    #[error("a photo entry is not encrypted")]
    NotEncrypted,
    #[error("encode: {0}")]
    Encode(String),
    #[error("signature verification failed")]
    VerificationFailed,
}

/// Address of an album contract: `BLAKE3(owner_vk ‖ album_id)[..16]`.
///
/// Derived rather than random so the owner can always recompute it, and
/// address-based rather than key-based so **no key material sits in contract
/// parameters** — parameters are public and permanent, and a key placed
/// there could never be rotated.
pub fn address_of(owner_vk: &[u8], album_id: u32) -> [u8; ADDRESS_BYTES] {
    let mut h = blake3::Hasher::new();
    h.update(b"lkng/album-address/v1");
    h.update(owner_vk);
    h.update(&album_id.to_le_bytes());
    let full = h.finalize();
    let mut out = [0u8; ADDRESS_BYTES];
    out.copy_from_slice(&full.as_bytes()[..ADDRESS_BYTES]);
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumParams {
    pub schema_v: u8,
    pub address: [u8; ADDRESS_BYTES],
}

/// One encrypted photo.
///
/// There is no plaintext variant of this type, deliberately. A struct that
/// *could* hold a cleartext photo is a struct someone will eventually
/// serialise into a contract by mistake, and on this network that mistake is
/// permanent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedPhoto {
    /// XChaCha20-Poly1305 nonce.
    #[serde(with = "serde_bytes")]
    pub nonce: Vec<u8>,
    /// Ciphertext, including the Poly1305 tag.
    #[serde(with = "serde_bytes")]
    pub ciphertext: Vec<u8>,
    /// Which key generation encrypted this.
    ///
    /// Incremented when someone is removed. A viewer holding generation 3
    /// can read generations 1..=3 and nothing after — which is exactly what
    /// "revocation is prospective" means, made explicit in the data rather
    /// than left as a claim in a document.
    pub generation: u32,
    /// Owner-claimed time; ordering only.
    pub added_ms: u64,
}

impl EncryptedPhoto {
    pub fn validate(&self) -> Result<(), AlbumError> {
        if self.ciphertext.len() > MAX_PHOTO_BYTES {
            return Err(AlbumError::PhotoTooLarge);
        }
        // A 24-byte nonce and a non-empty ciphertext are the minimum
        // evidence that this went through the cipher at all. It is a cheap
        // check against the one mistake that cannot be undone.
        if self.nonce.len() != 24 || self.ciphertext.is_empty() {
            return Err(AlbumError::NotEncrypted);
        }
        Ok(())
    }
}

/// The album, as it exists on the network: ciphertext and a signature.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AlbumState {
    /// Owner's durable verifying key.
    ///
    /// Durable, unlike a tile's, because an album is a long-lived thing its
    /// owner returns to. It is not a linkability leak in the way a tile
    /// would be: an album's address is only known to people the owner
    /// granted it to, so this key is not lying in a public cell for
    /// scrapers.
    #[serde(default, with = "serde_bytes")]
    pub owner_vk: Option<Vec<u8>>,
    pub photos: BTreeMap<[u8; 32], EncryptedPhoto>,
    /// Current key generation.
    pub generation: u32,
    #[serde(default, with = "serde_bytes")]
    pub sig: Option<Vec<u8>>,
}

impl AlbumState {
    pub fn validate(&self) -> Result<(), AlbumError> {
        if self.photos.len() > MAX_PHOTOS {
            return Err(AlbumError::TooManyPhotos);
        }
        for p in self.photos.values() {
            p.validate()?;
        }
        if let Some(vk) = &self.owner_vk {
            if vk.len() != ML_DSA_65_VK_BYTES {
                return Err(AlbumError::BadOwnerKey);
            }
        }
        if let Some(sig) = &self.sig {
            if sig.is_empty() || sig.len() > MAX_SIG_BYTES {
                return Err(AlbumError::MalformedSignature);
            }
        }
        Ok(())
    }

    /// Bytes the owner signs: everything except the signature itself.
    ///
    /// Single-writer, so the whole state is signed rather than each photo —
    /// unlike the presence cell, where anyone may write and per-record
    /// signatures are the only way to tell contributors apart.
    pub fn signing_payload(&self, params: &AlbumParams) -> Result<Vec<u8>, AlbumError> {
        #[derive(Serialize)]
        struct Scoped<'a> {
            domain: &'a str,
            schema_v: u8,
            address: &'a [u8; ADDRESS_BYTES],
            owner_vk: Option<&'a [u8]>,
            photos: &'a BTreeMap<[u8; 32], EncryptedPhoto>,
            generation: u32,
        }
        let scoped = Scoped {
            domain: SIG_DOMAIN,
            schema_v: params.schema_v,
            address: &params.address,
            owner_vk: self.owner_vk.as_deref(),
            photos: &self.photos,
            generation: self.generation,
        };
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&scoped, &mut buf)
            .map_err(|e| AlbumError::Encode(e.to_string()))?;
        Ok(buf)
    }

    pub fn insert(&mut self, photo: EncryptedPhoto) {
        let mut h = blake3::Hasher::new();
        h.update(&photo.nonce);
        h.update(&photo.ciphertext);
        self.photos.insert(*h.finalize().as_bytes(), photo);
    }

    /// Photos a viewer holding `generation` can decrypt.
    ///
    /// Everything at or below their generation, nothing above. This is where
    /// "revocation is prospective" is actually implemented; the doc comment
    /// at the top of this module is only a description of this function.
    pub fn readable_at(&self, generation: u32) -> Vec<&EncryptedPhoto> {
        let mut v: Vec<&EncryptedPhoto> = self
            .photos
            .values()
            .filter(|p| p.generation <= generation)
            .collect();
        v.sort_by_key(|p| p.added_ms);
        v
    }
}

/// What the owner sends a viewer so they can read the album.
///
/// Travels inside an ordinary sealed inbox envelope, so on the wire it is
/// indistinguishable from a message: nobody replicating an inbox can tell
/// that an album was shared, with whom, or that one exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub address: [u8; ADDRESS_BYTES],
    #[serde(with = "serde_bytes")]
    pub key: Vec<u8>,
    /// The generation this key opens. A grant is not a permanent right; it
    /// is a right to what existed when it was given, plus whatever is added
    /// while it remains current.
    pub generation: u32,
    /// Owner's durable verifying key, so the recipient can check the album
    /// they are pointed at is signed by whoever granted it — otherwise a
    /// grant is an invitation to fetch a stranger's contract.
    #[serde(with = "serde_bytes")]
    pub owner_vk: Vec<u8>,
}

impl Grant {
    pub fn encode(&self) -> Result<Vec<u8>, AlbumError> {
        let mut buf = GRANT_DOMAIN.to_vec();
        ciborium::ser::into_writer(self, &mut buf)
            .map_err(|e| AlbumError::Encode(e.to_string()))?;
        Ok(buf)
    }

    /// Decode a grant, returning `None` for anything that is not one.
    ///
    /// The domain prefix means an ordinary text message can never be
    /// mistaken for a grant, and — more importantly — a grant can never be
    /// rendered as if it were text a person typed.
    pub fn decode(bytes: &[u8]) -> Option<Grant> {
        let rest = bytes.strip_prefix(GRANT_DOMAIN)?;
        let g: Grant = ciborium::de::from_reader(rest).ok()?;
        (g.key.len() == KEY_BYTES).then_some(g)
    }
}

#[cfg(feature = "verify")]
pub mod verify {
    use super::*;
    use ml_dsa::{MlDsa65, Signature, VerifyingKey};

    pub const SIGN_CONTEXT: &[u8] = b"lkng/v1";

    /// Verify an album is signed by the key it claims as owner.
    pub fn verify_album(state: &AlbumState, params: &AlbumParams) -> Result<(), AlbumError> {
        state.validate()?;
        let (Some(vk_bytes), Some(sig_bytes)) = (&state.owner_vk, &state.sig) else {
            // An unsigned, ownerless album is the empty starting state; it
            // says nothing and can be replaced by anyone's first write.
            return if state.photos.is_empty() {
                Ok(())
            } else {
                Err(AlbumError::VerificationFailed)
            };
        };

        // The address must be derivable from the claimed owner. Without this
        // anyone could sign their own album and place it at someone else's
        // address, and a grant would point viewers at an impostor.
        let mut matches = false;
        for album_id in 0..8u32 {
            if address_of(vk_bytes, album_id) == params.address {
                matches = true;
                break;
            }
        }
        if !matches {
            return Err(AlbumError::VerificationFailed);
        }

        let encoded: &[u8; ML_DSA_65_VK_BYTES] = vk_bytes[..]
            .try_into()
            .map_err(|_| AlbumError::BadOwnerKey)?;
        let vk = VerifyingKey::<MlDsa65>::decode(encoded.into());
        let sig_arr: &[u8; 3309] = sig_bytes[..]
            .try_into()
            .map_err(|_| AlbumError::MalformedSignature)?;
        let sig = Signature::<MlDsa65>::decode(sig_arr.into())
            .ok_or(AlbumError::MalformedSignature)?;
        let payload = state.signing_payload(params)?;
        if vk.verify_with_context(&payload, SIGN_CONTEXT, &sig) {
            Ok(())
        } else {
            Err(AlbumError::VerificationFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(gen: u32, added: u64, n: u8) -> EncryptedPhoto {
        EncryptedPhoto {
            nonce: vec![n; 24],
            ciphertext: vec![n; 128],
            generation: gen,
            added_ms: added,
        }
    }

    /// The property the whole design exists for.
    ///
    /// A viewer removed after generation 2 keeps what they could already
    /// read and gains nothing afterwards. If this ever returned the newer
    /// photo, "remove from album" would be a lie told to the owner.
    #[test]
    fn a_removed_viewer_sees_nothing_added_afterwards() {
        let mut a = AlbumState::default();
        a.insert(photo(1, 10, 1));
        a.insert(photo(2, 20, 2));
        a.insert(photo(3, 30, 3)); // added after they were removed
        a.generation = 3;

        let theirs = a.readable_at(2);
        assert_eq!(theirs.len(), 2, "they keep what they could already read");
        assert!(
            theirs.iter().all(|p| p.generation <= 2),
            "and gain nothing added after removal"
        );
    }

    /// ...and the honest converse: they do NOT lose what they had.
    ///
    /// Asserted so nobody later "fixes" `readable_at` into pretending
    /// otherwise. The bytes are on their device; a data structure cannot
    /// take them back, and the UI must not imply it can.
    #[test]
    fn removal_does_not_take_back_what_was_already_shared() {
        let mut a = AlbumState::default();
        a.insert(photo(1, 10, 1));
        a.generation = 5;
        assert_eq!(
            a.readable_at(1).len(),
            1,
            "a photo already shared stays readable -- revocation is prospective"
        );
    }

    #[test]
    fn an_unencrypted_photo_is_rejected() {
        let mut p = photo(1, 1, 1);
        p.nonce = vec![];
        assert_eq!(p.validate(), Err(AlbumError::NotEncrypted));

        let mut q = photo(1, 1, 1);
        q.ciphertext = vec![];
        assert_eq!(q.validate(), Err(AlbumError::NotEncrypted));
    }

    #[test]
    fn an_oversized_photo_is_rejected() {
        let mut p = photo(1, 1, 1);
        p.ciphertext = vec![0; MAX_PHOTO_BYTES + 1];
        assert_eq!(p.validate(), Err(AlbumError::PhotoTooLarge));
    }

    #[test]
    fn a_grant_round_trips_and_rejects_anything_else() {
        let g = Grant {
            address: [7; ADDRESS_BYTES],
            key: vec![3; KEY_BYTES],
            generation: 2,
            owner_vk: vec![1; ML_DSA_65_VK_BYTES],
        };
        let bytes = g.encode().unwrap();
        assert_eq!(Grant::decode(&bytes), Some(g));

        // A plain message must never decode as a grant, and vice versa.
        assert_eq!(Grant::decode(b"hello, are you around?"), None);
        assert_eq!(Grant::decode(&[]), None);
    }

    /// A grant with a short key is not a grant.
    ///
    /// Cheap, but it is the check that stops a malformed or truncated grant
    /// being handed to the cipher, where a short key would either panic or —
    /// worse — be silently padded.
    #[test]
    fn a_grant_with_the_wrong_key_length_is_rejected() {
        let g = Grant {
            address: [7; ADDRESS_BYTES],
            key: vec![3; 16],
            generation: 1,
            owner_vk: vec![1; ML_DSA_65_VK_BYTES],
        };
        assert_eq!(Grant::decode(&g.encode().unwrap()), None);
    }

    #[test]
    fn addresses_differ_per_owner_and_per_album() {
        let a = address_of(&[1; ML_DSA_65_VK_BYTES], 0);
        let b = address_of(&[1; ML_DSA_65_VK_BYTES], 1);
        let c = address_of(&[2; ML_DSA_65_VK_BYTES], 0);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, address_of(&[1; ML_DSA_65_VK_BYTES], 0), "and are stable");
    }

    #[test]
    fn the_album_cap_is_enforced() {
        let mut a = AlbumState::default();
        for i in 0..(MAX_PHOTOS as u8 + 2) {
            a.insert(photo(1, i as u64, i));
        }
        assert_eq!(a.validate(), Err(AlbumError::TooManyPhotos));
    }
}
