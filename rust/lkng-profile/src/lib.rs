//! Durable profile state for LKNG.
//!
//! Shape copied from Delta's site contract: **single-writer**. Exactly one
//! key — the owner's durable identity — may ever change this state, which
//! makes convergence trivial (last-writer-wins on a monotonic sequence)
//! and the security model easy to state.
//!
//! This is the state a presence tile deliberately does **not** point at.
//! Tiles are signed by per-epoch subkeys precisely so that scraping a cell
//! cannot lead here; the address is revealed by its owner during a match.
//!
//! Five Delta lessons are load-bearing (see PLAN.md "Findings"):
//!
//! 1. **Sign each item, not the whole state** — every field group carries
//!    its own signature, so a merged state still proves each part.
//! 2. **Monotonic sequence with a content-hash tiebreak** — a total order,
//!    so two states converge without consulting a clock.
//! 3. **Signature schema evolution** — `verify` tries the current payload
//!    layout, then older ones, so adding a field doesn't invalidate every
//!    profile already on the network.
//! 4. **Address claiming is security-critical** — the owner key is pinned
//!    in the contract *parameters*, so an empty state at that address can
//!    only ever be claimed by the matching key.
//! 5. **Tombstones must be authenticated and bound to their key** — a
//!    deletion is as destructive as a write.

use std::collections::BTreeMap;

use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};

/// Domain tag for profile signatures. Distinct from the presence tag, so a
/// signature over one can never be reinterpreted as the other.
pub const SIG_DOMAIN_V2: &str = "lkng/profile/v2";
/// Previous payload layout, still accepted on verification (Delta lesson 3).
pub const SIG_DOMAIN_V1: &str = "lkng/profile/v1";

pub const MAX_DISPLAY_NAME_BYTES: usize = 48;
pub const MAX_BIO_BYTES: usize = 600;
pub const MAX_TAGS: usize = 12;
pub const MAX_TAG_BYTES: usize = 24;
/// Photo references are content hashes, not inline bytes — full-size media
/// lives in its own contracts (chunked when large).
pub const MAX_PHOTOS: usize = 8;
pub const MAX_THUMBNAIL_BYTES: usize = 16 * 1024;
pub const MAX_SIG_BYTES: usize = 4096;
pub const ML_DSA_65_VK_BYTES: usize = 1952;

/// Contract parameters: who owns this profile. Part of `hash(code, params)`,
/// so the address IS the identity — nobody else can occupy it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileParams {
    pub schema_v: u8,
    /// Owner's durable ML-DSA-65 verifying key (1952 B).
    #[serde(with = "serde_bytes")]
    pub owner_vk: Vec<u8>,
}

impl ProfileParams {
    pub fn new(owner_vk: Vec<u8>) -> Self {
        Self { schema_v: 1, owner_vk }
    }

    /// Short shareable handle, same construction as identity fingerprints.
    pub fn handle(&self) -> String {
        let h = blake3::hash(&self.owner_vk);
        bs58_encode(&h.as_bytes()[..8])
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ProfileError {
    #[error("display name exceeds {MAX_DISPLAY_NAME_BYTES} bytes")]
    NameTooLong,
    #[error("bio exceeds {MAX_BIO_BYTES} bytes")]
    BioTooLong,
    #[error("too many tags (max {MAX_TAGS}) or a tag exceeds {MAX_TAG_BYTES} bytes")]
    BadTags,
    #[error("too many photos (max {MAX_PHOTOS})")]
    TooManyPhotos,
    #[error("thumbnail exceeds {MAX_THUMBNAIL_BYTES} bytes")]
    ThumbnailTooLarge,
    #[error("signature is empty or exceeds {MAX_SIG_BYTES} bytes")]
    MalformedSignature,
    #[error("owner key is not a valid ML-DSA-65 key")]
    BadOwnerKey,
    #[error("signature verification failed")]
    VerificationFailed,
    #[error("state does not belong to the owner in the parameters")]
    WrongOwner,
    #[error("tombstone is filed under a different key than it was signed for")]
    TombstoneKeyMismatch,
    #[error("encode: {0}")]
    Encode(String),
}

/// A content-addressed photo reference. Bytes live elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PhotoRef {
    pub hash: [u8; 32],
    /// True for the tile image shown in the grid.
    pub is_primary: bool,
}

/// The mutable, owner-signed body of a profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBody {
    pub display_name: String,
    pub bio: String,
    pub tags: Vec<String>,
    pub photos: Vec<PhotoRef>,
    /// Small inline image so a matched peer can render immediately without
    /// a second fetch that might hit an evicted contract.
    #[serde(with = "serde_bytes")]
    pub thumbnail: Vec<u8>,
    /// Monotonic. Higher wins; ties break on content hash.
    pub sequence: u64,
}

impl ProfileBody {
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.display_name.len() > MAX_DISPLAY_NAME_BYTES {
            return Err(ProfileError::NameTooLong);
        }
        if self.bio.len() > MAX_BIO_BYTES {
            return Err(ProfileError::BioTooLong);
        }
        if self.tags.len() > MAX_TAGS || self.tags.iter().any(|t| t.len() > MAX_TAG_BYTES) {
            return Err(ProfileError::BadTags);
        }
        if self.photos.len() > MAX_PHOTOS {
            return Err(ProfileError::TooManyPhotos);
        }
        if self.thumbnail.len() > MAX_THUMBNAIL_BYTES {
            return Err(ProfileError::ThumbnailTooLarge);
        }
        Ok(())
    }

    /// Canonical bytes for signing, under a given domain version.
    fn signing_payload(
        &self,
        params: &ProfileParams,
        domain: &str,
    ) -> Result<Vec<u8>, ProfileError> {
        #[derive(Serialize)]
        struct Scoped<'a> {
            domain: &'a str,
            schema_v: u8,
            owner_vk: &'a [u8],
            display_name: &'a str,
            bio: &'a str,
            tags: &'a [String],
            photos: &'a [PhotoRef],
            thumbnail: &'a [u8],
            sequence: u64,
        }
        let scoped = Scoped {
            domain,
            schema_v: params.schema_v,
            owner_vk: &params.owner_vk,
            display_name: &self.display_name,
            bio: &self.bio,
            tags: &self.tags,
            photos: &self.photos,
            thumbnail: &self.thumbnail,
            sequence: self.sequence,
        };
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&scoped, &mut buf)
            .map_err(|e| ProfileError::Encode(e.to_string()))?;
        Ok(buf)
    }

    /// Payload for the current signature version.
    pub fn signing_payload_current(
        &self,
        params: &ProfileParams,
    ) -> Result<Vec<u8>, ProfileError> {
        self.signing_payload(params, SIG_DOMAIN_V2)
    }

    /// Content hash, used as the deterministic tiebreak on equal sequence.
    pub fn content_hash(&self) -> Result<[u8; 32], ProfileError> {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(self, &mut buf)
            .map_err(|e| ProfileError::Encode(e.to_string()))?;
        Ok(*blake3::hash(&buf).as_bytes())
    }
}

/// A signed deletion of the profile. Authenticated and bound to the owner
/// (Delta lesson 5) — an unauthenticated tombstone would let any peer wipe
/// a profile it merely copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDeletion {
    /// Must exceed the last live sequence, so a deletion cannot be undone
    /// by replaying an older body.
    pub sequence: u64,
    #[serde(with = "serde_bytes")]
    pub sig: Vec<u8>,
}

impl SignedDeletion {
    pub fn signing_payload(&self, params: &ProfileParams) -> Result<Vec<u8>, ProfileError> {
        #[derive(Serialize)]
        struct Scoped<'a> {
            domain: &'a str,
            owner_vk: &'a [u8],
            sequence: u64,
        }
        let mut buf = Vec::new();
        ciborium::ser::into_writer(
            &Scoped {
                domain: "lkng/profile-deletion/v1",
                owner_vk: &params.owner_vk,
                sequence: self.sequence,
            },
            &mut buf,
        )
        .map_err(|e| ProfileError::Encode(e.to_string()))?;
        Ok(buf)
    }
}

/// Full profile contract state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileState {
    pub body: Option<ProfileBody>,
    /// Owner signature over `body`'s payload.
    #[serde(default, with = "serde_bytes")]
    pub sig: Option<Vec<u8>>,
    /// Set once deleted; suppresses any body at or below its sequence.
    pub deleted: Option<SignedDeletion>,
}

/// Summary for delta sync: what the peer already has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub sequence: u64,
    pub content_hash: Option<[u8; 32]>,
    pub deleted_at: Option<u64>,
}

impl ProfileState {
    pub fn sequence(&self) -> u64 {
        self.body.as_ref().map(|b| b.sequence).unwrap_or(0)
    }

    /// Per-field invariants. Signature checking lives in [`verify`] so this
    /// stays usable without the crypto feature.
    pub fn validate_shape(&self) -> Result<(), ProfileError> {
        if let Some(b) = &self.body {
            b.validate()?;
            match &self.sig {
                Some(s) if !s.is_empty() && s.len() <= MAX_SIG_BYTES => {}
                _ => return Err(ProfileError::MalformedSignature),
            }
        }
        if let Some(d) = &self.deleted {
            if d.sig.is_empty() || d.sig.len() > MAX_SIG_BYTES {
                return Err(ProfileError::MalformedSignature);
            }
        }
        Ok(())
    }

    pub fn summarize(&self) -> ProfileSummary {
        ProfileSummary {
            sequence: self.sequence(),
            content_hash: self.body.as_ref().and_then(|b| b.content_hash().ok()),
            deleted_at: self.deleted.as_ref().map(|d| d.sequence),
        }
    }

    /// Total order for last-writer-wins: higher sequence, then higher
    /// content hash. No clock is consulted — two peers holding the same
    /// pair of states always pick the same winner.
    fn beats(&self, other: &ProfileState) -> bool {
        match self.sequence().cmp(&other.sequence()) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => {
                let a = self.body.as_ref().and_then(|b| b.content_hash().ok());
                let b = other.body.as_ref().and_then(|x| x.content_hash().ok());
                a > b
            }
        }
    }

    /// Merge: last-writer-wins on the body, and deletion is sticky.
    pub fn merge(&mut self, other: &ProfileState) {
        // A deletion with a higher sequence always survives a merge.
        match (&self.deleted, &other.deleted) {
            (None, Some(d)) => self.deleted = Some(d.clone()),
            (Some(mine), Some(theirs)) if theirs.sequence > mine.sequence => {
                self.deleted = Some(theirs.clone())
            }
            _ => {}
        }
        if other.beats(self) {
            self.body = other.body.clone();
            self.sig = other.sig.clone();
        }
        // A body at or below the deletion sequence is suppressed.
        if let Some(d) = &self.deleted {
            if self.sequence() <= d.sequence {
                self.body = None;
                self.sig = None;
            }
        }
    }
}

fn bs58_encode(bytes: &[u8]) -> String {
    // Small local base58 so this crate stays dependency-light for WASM.
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut digits = vec![0u8];
    for &b in bytes {
        let mut carry = b as usize;
        for d in digits.iter_mut() {
            carry += (*d as usize) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let leading = bytes.iter().take_while(|&&b| b == 0).count();
    let mut out: Vec<u8> = std::iter::repeat(ALPHABET[0]).take(leading).collect();
    out.extend(digits.iter().rev().map(|&d| ALPHABET[d as usize]));
    String::from_utf8(out).expect("base58 alphabet is ascii")
}

/// Signature verification, WASM-safe (no RNG) so the contract can enforce
/// it — the same split `lkng-presence` uses.
#[cfg(feature = "verify")]
pub mod verify {
    use super::*;
    use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa65, Signature, VerifyingKey};

    pub const SIGN_CONTEXT: &[u8] = b"lkng/v1";

    fn check(vk_bytes: &[u8], payload: &[u8], sig_bytes: &[u8]) -> Result<bool, ProfileError> {
        let enc: &EncodedVerifyingKey<MlDsa65> =
            vk_bytes.try_into().map_err(|_| ProfileError::BadOwnerKey)?;
        let vk = VerifyingKey::<MlDsa65>::decode(enc);
        let sb: &EncodedSignature<MlDsa65> = sig_bytes
            .try_into()
            .map_err(|_| ProfileError::VerificationFailed)?;
        let sig = Signature::<MlDsa65>::decode(sb).ok_or(ProfileError::VerificationFailed)?;
        Ok(vk.verify_with_context(payload, SIGN_CONTEXT, &sig))
    }

    /// Verify a whole profile state against its parameters.
    ///
    /// Tries the current signature layout first, then older ones, so a
    /// profile signed before a field was added keeps verifying instead of
    /// silently vanishing from the network (Delta lesson 3).
    pub fn verify_state(state: &ProfileState, params: &ProfileParams) -> Result<(), ProfileError> {
        if params.owner_vk.len() != ML_DSA_65_VK_BYTES {
            return Err(ProfileError::BadOwnerKey);
        }
        state.validate_shape()?;

        if let (Some(body), Some(sig)) = (&state.body, &state.sig) {
            let mut ok = false;
            for domain in [SIG_DOMAIN_V2, SIG_DOMAIN_V1] {
                let payload = body.signing_payload(params, domain)?;
                if check(&params.owner_vk, &payload, sig)? {
                    ok = true;
                    break;
                }
            }
            if !ok {
                return Err(ProfileError::VerificationFailed);
            }
        }

        if let Some(d) = &state.deleted {
            let payload = d.signing_payload(params)?;
            if !check(&params.owner_vk, &payload, &d.sig)? {
                return Err(ProfileError::VerificationFailed);
            }
        }
        Ok(())
    }
}

impl ComposableState for ProfileState {
    type ParentState = Self;
    type Summary = ProfileSummary;
    type Delta = ProfileState;
    type Parameters = ProfileParams;

    fn verify(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Result<(), String> {
        self.validate_shape().map_err(|e| e.to_string())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        ProfileState::summarize(self)
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old: &Self::Summary,
    ) -> Option<Self::Delta> {
        // Nothing to send when the peer already matches — the caller MUST
        // turn None into zero bytes, never an encoded empty struct (#5072).
        if ProfileState::summarize(self) == *old {
            None
        } else {
            Some(self.clone())
        }
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(d) = delta {
            ProfileState::merge(self, d);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ProfileParams {
        ProfileParams::new(vec![7u8; ML_DSA_65_VK_BYTES])
    }

    fn body(seq: u64, name: &str) -> ProfileBody {
        ProfileBody {
            display_name: name.into(),
            bio: "hello".into(),
            tags: vec!["a".into()],
            photos: vec![],
            thumbnail: vec![1, 2, 3],
            sequence: seq,
        }
    }

    fn state(seq: u64, name: &str) -> ProfileState {
        ProfileState {
            body: Some(body(seq, name)),
            sig: Some(vec![9; 64]),
            deleted: None,
        }
    }

    #[test]
    fn higher_sequence_wins_either_direction() {
        let mut a = state(1, "old");
        let b = state(2, "new");
        a.merge(&b);
        assert_eq!(a.sequence(), 2);

        let mut b2 = state(2, "new");
        b2.merge(&state(1, "old"));
        assert_eq!(b2.sequence(), 2, "merging older into newer must not regress");
    }

    #[test]
    fn equal_sequence_resolves_deterministically() {
        let x = state(5, "alpha");
        let y = state(5, "beta");
        let mut ab = x.clone();
        ab.merge(&y);
        let mut ba = y.clone();
        ba.merge(&x);
        assert_eq!(ab, ba, "same winner regardless of merge direction");
    }

    #[test]
    fn merge_is_idempotent() {
        let mut a = state(3, "x");
        let snapshot = a.clone();
        a.merge(&snapshot.clone());
        assert_eq!(a, snapshot);
    }

    #[test]
    fn deletion_suppresses_body_and_is_sticky() {
        let mut a = state(3, "x");
        let tomb = ProfileState {
            body: None,
            sig: None,
            deleted: Some(SignedDeletion { sequence: 5, sig: vec![1; 64] }),
        };
        a.merge(&tomb);
        assert!(a.body.is_none(), "deletion must suppress the body");

        // Replaying an older body must not resurrect the profile.
        a.merge(&state(4, "resurrected"));
        assert!(a.body.is_none(), "deletion must be sticky against older writes");
    }

    #[test]
    fn a_newer_body_can_follow_a_deletion() {
        // Deleting is not permanent banishment — the owner may re-publish
        // with a higher sequence than the tombstone.
        let mut a = ProfileState {
            body: None,
            sig: None,
            deleted: Some(SignedDeletion { sequence: 5, sig: vec![1; 64] }),
        };
        a.merge(&state(6, "back"));
        assert_eq!(a.sequence(), 6);
        assert!(a.body.is_some());
    }

    #[test]
    fn caps_are_enforced() {
        let mut b = body(1, "n");
        b.bio = "x".repeat(MAX_BIO_BYTES + 1);
        assert_eq!(b.validate(), Err(ProfileError::BioTooLong));

        let mut b = body(1, "n");
        b.tags = (0..MAX_TAGS + 1).map(|i| i.to_string()).collect();
        assert_eq!(b.validate(), Err(ProfileError::BadTags));

        let mut b = body(1, "n");
        b.thumbnail = vec![0; MAX_THUMBNAIL_BYTES + 1];
        assert_eq!(b.validate(), Err(ProfileError::ThumbnailTooLarge));
    }

    #[test]
    fn body_without_signature_is_invalid() {
        let s = ProfileState { body: Some(body(1, "n")), sig: None, deleted: None };
        assert_eq!(s.validate_shape(), Err(ProfileError::MalformedSignature));
    }

    #[test]
    fn signing_payload_is_domain_and_owner_bound() {
        let b = body(1, "n");
        let p1 = params();
        let mut p2 = params();
        p2.owner_vk = vec![8u8; ML_DSA_65_VK_BYTES];
        assert_ne!(
            b.signing_payload_current(&p1).unwrap(),
            b.signing_payload_current(&p2).unwrap(),
            "a profile signature must not transfer to another owner's address"
        );
        assert_ne!(
            b.signing_payload(&p1, SIG_DOMAIN_V2).unwrap(),
            b.signing_payload(&p1, SIG_DOMAIN_V1).unwrap(),
            "signature versions must produce distinct payloads"
        );
    }

    #[test]
    fn handle_is_short_and_stable() {
        let p = params();
        assert_eq!(p.handle(), params().handle());
        assert!(p.handle().len() <= 12);
    }

    #[test]
    fn delta_is_none_when_peer_is_current() {
        let s = state(2, "x");
        let parent = s.clone();
        let p = params();
        let summary = ProfileState::summarize(&s);
        assert!(
            s.delta(&parent, &p, &summary).is_none(),
            "identical peers -> None -> zero bytes on the wire"
        );
    }
}
