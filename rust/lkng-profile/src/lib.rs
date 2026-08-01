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
pub const X25519_KEY_BYTES: usize = 32;

/// Truncation length of an address, in bytes.
///
/// **A security parameter, not a UX one.** The property that matters is
/// second-preimage resistance: if an attacker can grind a *different*
/// keypair whose hash truncates to your address, they can stand up state
/// at your address carrying their key, and the binding check passes for
/// both. No contract-side tie-break rescues this — "first writer wins" is
/// unenforceable in a permissionless eventually-consistent store, and any
/// deterministic ordering on key bytes is itself grindable. 16 bytes gives
/// 128-bit resistance; length is the only real defence.
pub const ADDRESS_BYTES: usize = 16;

/// Address of an identity: truncated BLAKE3 of its **long-term signing
/// key only**.
///
/// Deliberately not derived from the encryption key as well: keeping the
/// encryption key out of the address is what lets a user rotate it without
/// their address — and every link anyone has to them — changing.
pub fn address_of(owner_vk: &[u8]) -> [u8; ADDRESS_BYTES] {
    blake3::hash(owner_vk).as_bytes()[..ADDRESS_BYTES]
        .try_into()
        .expect("slice length matches ADDRESS_BYTES")
}

/// Contract parameters: *just the address*.
///
/// The full ML-DSA-65 verifying key is 1952 bytes, and parameters are
/// carried by every client on every GET, PUT and subscribe — so key
/// material does not belong here. The key lives in state, and
/// `verify_state` rejects any state whose key does not hash to this
/// address, which keeps the identifier self-certifying: given an address
/// you can fetch the contract, read the key, and check it yourself with no
/// directory and no trusted lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileParams {
    pub schema_v: u8,
    pub address: [u8; ADDRESS_BYTES],
}

impl ProfileParams {
    pub fn new(owner_vk: impl AsRef<[u8]>) -> Self {
        Self { schema_v: 1, address: address_of(owner_vk.as_ref()) }
    }

    /// The shareable handle: base58 of the address. base58 is the *display*
    /// form only — parameters carry the raw bytes.
    pub fn handle(&self) -> String {
        bs58_encode(&self.address)
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
    #[error("encryption key must be 32 bytes (X25519)")]
    BadEncryptionKey,
    #[error("age must be between {MIN_AGE} and {MAX_AGE}")]
    BadAge,
    #[error("a demographic field is out of range or too long")]
    BadDemographic,
    #[error("signature verification failed")]
    VerificationFailed,
    #[error("state does not belong to the owner in the parameters")]
    WrongOwner,
    #[error("tombstone is filed under a different key than it was signed for")]
    TombstoneKeyMismatch,
    #[error("encode: {0}")]
    Encode(String),
}

/// Gender identity.
///
/// An enum with an explicit `SelfDescribed` escape hatch rather than a
/// closed list: a fixed set always fails somebody, and in this app failing
/// somebody's gender is not a cosmetic bug. The named variants exist so
/// filtering is reliable for the common cases; the free-text variant means
/// nobody is forced into a box that is wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gender {
    Man,
    Woman,
    NonBinary,
    TransMan,
    TransWoman,
    /// Anything else, in the person's own words.
    SelfDescribed(String),
}

impl Gender {
    pub fn label(&self) -> String {
        match self {
            Gender::Man => "Man".into(),
            Gender::Woman => "Woman".into(),
            Gender::NonBinary => "Non-binary".into(),
            Gender::TransMan => "Trans man".into(),
            Gender::TransWoman => "Trans woman".into(),
            Gender::SelfDescribed(s) => s.clone(),
        }
    }

    fn valid(&self) -> bool {
        match self {
            Gender::SelfDescribed(s) => !s.is_empty() && s.len() <= MAX_DEMOGRAPHIC_BYTES,
            _ => true,
        }
    }
}

/// Sexual position. Ordinary preference data, not health data, so this is
/// allowed on a public tile as well as in a profile — it is also the
/// filter people reach for most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Position {
    Top,
    VerseTop,
    Versatile,
    VerseBottom,
    Bottom,
    Side,
}

impl Position {
    /// Compact wire/tile representation. 0 is reserved for "unstated", so
    /// an absent value is never confused with `Top`.
    pub fn code(self) -> u8 {
        match self {
            Position::Top => 1,
            Position::VerseTop => 2,
            Position::Versatile => 3,
            Position::VerseBottom => 4,
            Position::Bottom => 5,
            Position::Side => 6,
        }
    }

    pub fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            1 => Position::Top,
            2 => Position::VerseTop,
            3 => Position::Versatile,
            4 => Position::VerseBottom,
            5 => Position::Bottom,
            6 => Position::Side,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Position::Top => "Top",
            Position::VerseTop => "Verse-Top",
            Position::Versatile => "Versatile",
            Position::VerseBottom => "Verse-Bottom",
            Position::Bottom => "Bottom",
            Position::Side => "Side",
        }
    }
}

/// HIV status, using current clinical vocabulary.
///
/// # This never goes on a tile
///
/// Health data on a presence tile would be public to anyone subscribing to
/// a cell and permanently harvestable — a standing list of who in a
/// neighbourhood is positive. HIV status is criminalised or grounds for
/// discrimination in many jurisdictions, and this exact data has been
/// mishandled by a major app in this category before. It lives **only** in
/// a profile, which its owner reveals to someone specific.
/// `hiv_status_never_reaches_a_tile` enforces this rather than trusting a
/// convention.
///
/// `NegativeOnPrep` and `PositiveUndetectable` exist because they are how
/// people actually describe themselves, and because "undetectable = 
/// untransmittable" is settled science that a Negative/Positive binary
/// erases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HivStatus {
    Negative,
    NegativeOnPrep,
    Positive,
    PositiveUndetectable,
}

impl HivStatus {
    pub fn label(self) -> &'static str {
        match self {
            HivStatus::Negative => "Negative",
            HivStatus::NegativeOnPrep => "Negative, on PrEP",
            HivStatus::Positive => "Positive",
            HivStatus::PositiveUndetectable => "Positive, undetectable",
        }
    }
}

/// Structured demographics, for search and filtering.
///
/// These live in the **profile**, not the tile. A tile is public to anyone
/// scraping a cell; exact age, height, weight and ethnicity on a tile would
/// hand a scraper a rich dossier on everyone in a neighbourhood. The grid
/// filters on coarse bands instead (see `lkng_presence::TileFilters`), and
/// the precise values appear only in a profile its owner chose to share.
///
/// Every field is optional. Nobody should be forced to state their weight
/// to use a dating app, and a required field is a field people lie in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Demographics {
    #[serde(default)]
    pub age: Option<u8>,
    #[serde(default)]
    pub height_cm: Option<u16>,
    #[serde(default)]
    pub weight_kg: Option<u16>,
    /// Free text rather than an enum: a fixed list of ethnicities is a
    /// political statement that always excludes someone, and mixed-heritage
    /// people are ill-served by radio buttons.
    #[serde(default)]
    pub ethnicity: Option<String>,
    #[serde(default)]
    pub body_type: Option<String>,
    #[serde(default)]
    pub pronouns: Option<String>,
    #[serde(default)]
    pub looking_for: Option<String>,
    #[serde(default)]
    pub position: Option<Position>,
    #[serde(default)]
    pub gender: Option<Gender>,
    /// See [`HivStatus`]: profile only, never on a tile.
    #[serde(default)]
    pub hiv_status: Option<HivStatus>,
    /// Coarse "last tested" as `(year, month)`. Deliberately not a day:
    /// an exact date is one more correlator and adds nothing a filter
    /// needs.
    #[serde(default)]
    pub last_tested: Option<(u16, u8)>,
}

/// Longest any single demographic free-text field may be.
pub const MAX_DEMOGRAPHIC_BYTES: usize = 40;
/// Ages outside this are rejected outright: below is a child-safety matter,
/// above is certainly a typo or a joke, and both make filters meaningless.
pub const MIN_AGE: u8 = 18;
pub const MAX_AGE: u8 = 120;

impl Demographics {
    pub fn validate(&self) -> Result<(), ProfileError> {
        if let Some(a) = self.age {
            if !(MIN_AGE..=MAX_AGE).contains(&a) {
                return Err(ProfileError::BadAge);
            }
        }
        if let Some(h) = self.height_cm {
            if !(50..=280).contains(&h) {
                return Err(ProfileError::BadDemographic);
            }
        }
        if let Some(w) = self.weight_kg {
            if !(20..=400).contains(&w) {
                return Err(ProfileError::BadDemographic);
            }
        }
        if let Some(g) = &self.gender {
            if !g.valid() {
                return Err(ProfileError::BadDemographic);
            }
        }
        if let Some((y, m)) = self.last_tested {
            if !(1980..=2200).contains(&y) || !(1..=12).contains(&m) {
                return Err(ProfileError::BadDemographic);
            }
        }
        for f in [&self.ethnicity, &self.body_type, &self.pronouns, &self.looking_for] {
            if let Some(v) = f {
                if v.len() > MAX_DEMOGRAPHIC_BYTES {
                    return Err(ProfileError::BadDemographic);
                }
            }
        }
        Ok(())
    }

    /// Does this profile match a search? All supplied criteria must hold;
    /// an absent value never matches a criterion that requires it, so
    /// filtering never silently includes people who said nothing.
    pub fn matches(&self, q: &Search) -> bool {
        if let Some((lo, hi)) = q.age_range {
            match self.age {
                Some(a) if a >= lo && a <= hi => {}
                _ => return false,
            }
        }
        if let Some((lo, hi)) = q.height_cm_range {
            match self.height_cm {
                Some(h) if h >= lo && h <= hi => {}
                _ => return false,
            }
        }
        if !q.genders.is_empty() {
            match &self.gender {
                Some(g) if q.genders.contains(g) => {}
                _ => return false,
            }
        }
        if !q.positions.is_empty() {
            match self.position {
                Some(p) if q.positions.contains(&p) => {}
                _ => return false,
            }
        }
        // "Not specified" is a thing people deliberately search for, not
        // only a gap. Without this, someone who has not stated a position
        // is unreachable by any filtered search at all.
        if q.include_unspecified_position && self.position.is_none() {
            return true;
        }
        if !q.hiv_statuses.is_empty() {
            match self.hiv_status {
                Some(h) if q.hiv_statuses.contains(&h) => {}
                _ => return false,
            }
        }
        for (needle, field) in [
            (&q.ethnicity, &self.ethnicity),
            (&q.body_type, &self.body_type),
            (&q.looking_for, &self.looking_for),
        ] {
            if let Some(n) = needle {
                match field {
                    Some(v) if v.to_lowercase().contains(&n.to_lowercase()) => {}
                    _ => return false,
                }
            }
        }
        true
    }
}

/// A profile search. Every field is optional; an empty search matches all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub age_range: Option<(u8, u8)>,
    pub height_cm_range: Option<(u16, u16)>,
    pub ethnicity: Option<String>,
    pub body_type: Option<String>,
    pub looking_for: Option<String>,
    /// Empty means "any".
    pub genders: Vec<Gender>,
    /// Empty means "any". Listed rather than a single value because people
    /// search for several at once.
    pub positions: Vec<Position>,
    /// Deliberately include people who stated no position — the
    /// "Not specified" chip, distinct from "any".
    pub include_unspecified_position: bool,
    /// Empty means "any".
    pub hiv_statuses: Vec<HivStatus>,
    /// Substring match over display name, bio and tags.
    pub text: Option<String>,
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
    /// Structured, searchable demographics.
    #[serde(default)]
    pub demographics: Demographics,
    /// X25519 public key others use to seal messages to this identity.
    ///
    /// Published here rather than in a presence tile on purpose: a tile is
    /// public to anyone scraping a cell, and an encryption key that
    /// travelled with it would be one more durable handle to correlate.
    /// You can only write to someone whose profile they chose to show you.
    #[serde(default, with = "serde_bytes")]
    pub encryption_key: Option<Vec<u8>>,
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
        if let Some(k) = &self.encryption_key {
            if k.len() != X25519_KEY_BYTES {
                return Err(ProfileError::BadEncryptionKey);
            }
        }
        self.demographics.validate()?;
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
            address: &'a [u8],
            display_name: &'a str,
            bio: &'a str,
            tags: &'a [String],
            photos: &'a [PhotoRef],
            thumbnail: &'a [u8],
            encryption_key: Option<&'a [u8]>,
            demographics: &'a Demographics,
            sequence: u64,
        }
        let scoped = Scoped {
            domain,
            schema_v: params.schema_v,
            address: &params.address,
            display_name: &self.display_name,
            bio: &self.bio,
            tags: &self.tags,
            photos: &self.photos,
            thumbnail: &self.thumbnail,
            encryption_key: self.encryption_key.as_deref(),
            demographics: &self.demographics,
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

impl ProfileBody {
    /// Does this profile match a search, including free-text over name,
    /// bio and tags?
    pub fn matches(&self, q: &Search) -> bool {
        if !self.demographics.matches(q) {
            return false;
        }
        if let Some(t) = &q.text {
            let t = t.to_lowercase();
            let hay = format!(
                "{} {} {}",
                self.display_name.to_lowercase(),
                self.bio.to_lowercase(),
                self.tags.join(" ").to_lowercase()
            );
            if !hay.contains(&t) {
                return false;
            }
        }
        true
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
            address: &'a [u8],
            sequence: u64,
        }
        let mut buf = Vec::new();
        ciborium::ser::into_writer(
            &Scoped {
                domain: "lkng/profile-deletion/v1",
                address: &params.address,
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
    /// The long-term signing key this address commits to. Lives in state,
    /// never in parameters — see [`ProfileParams`].
    #[serde(default, with = "serde_bytes")]
    pub owner_vk: Vec<u8>,
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
        if state.owner_vk.len() != ML_DSA_65_VK_BYTES {
            return Err(ProfileError::BadOwnerKey);
        }
        // Bind state to the address. Without this an untrusted peer could
        // serve a contract at this address carrying somebody else's key.
        if address_of(&state.owner_vk) != params.address {
            return Err(ProfileError::WrongOwner);
        }
        state.validate_shape()?;

        if let (Some(body), Some(sig)) = (&state.body, &state.sig) {
            let mut ok = false;
            for domain in [SIG_DOMAIN_V2, SIG_DOMAIN_V1] {
                let payload = body.signing_payload(params, domain)?;
                if check(&state.owner_vk, &payload, sig)? {
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
            if !check(&state.owner_vk, &payload, &d.sig)? {
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
            demographics: Default::default(),
            encryption_key: None,
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

#[cfg(test)]
mod search_tests {
    use super::*;

    fn demo(age: u8, h: u16, eth: &str, looking: &str) -> Demographics {
        Demographics {
            age: Some(age),
            height_cm: Some(h),
            weight_kg: Some(75),
            ethnicity: Some(eth.into()),
            body_type: Some("average".into()),
            pronouns: Some("he/him".into()),
            looking_for: Some(looking.into()),
            position: None,
            gender: None,
            hiv_status: None,
            last_tested: None,
        }
    }

    #[test]
    fn empty_search_matches_everyone() {
        assert!(demo(30, 180, "latino", "dates").matches(&Search::default()));
        assert!(Demographics::default().matches(&Search::default()));
    }

    #[test]
    fn age_range_filters() {
        let d = demo(30, 180, "latino", "dates");
        assert!(d.matches(&Search { age_range: Some((25, 35)), ..Default::default() }));
        assert!(!d.matches(&Search { age_range: Some((18, 25)), ..Default::default() }));
    }

    #[test]
    fn unstated_values_never_match_a_criterion() {
        // Someone who declined to state their age must not be swept into
        // an age-filtered result set — filtering has to mean what it says.
        let quiet = Demographics::default();
        assert!(!quiet.matches(&Search { age_range: Some((18, 99)), ..Default::default() }));
    }

    #[test]
    fn text_fields_match_case_insensitively_on_substring() {
        let d = demo(30, 180, "Latino", "Dates");
        assert!(d.matches(&Search { ethnicity: Some("latin".into()), ..Default::default() }));
        assert!(d.matches(&Search { looking_for: Some("DATE".into()), ..Default::default() }));
        assert!(!d.matches(&Search { ethnicity: Some("nordic".into()), ..Default::default() }));
    }

    #[test]
    fn all_criteria_must_hold() {
        let d = demo(30, 180, "latino", "dates");
        let q = Search {
            age_range: Some((25, 35)),
            ethnicity: Some("nordic".into()),
            ..Default::default()
        };
        assert!(!d.matches(&q), "one failing criterion fails the whole search");
    }

    #[test]
    fn free_text_covers_name_bio_and_tags() {
        let body = ProfileBody {
            display_name: "Sam".into(),
            bio: "into bad horror films".into(),
            tags: vec!["cinema".into()],
            demographics: demo(30, 180, "latino", "dates"),
            sequence: 1,
            ..Default::default()
        };
        assert!(body.matches(&Search { text: Some("horror".into()), ..Default::default() }));
        assert!(body.matches(&Search { text: Some("cinema".into()), ..Default::default() }));
        assert!(body.matches(&Search { text: Some("sam".into()), ..Default::default() }));
        assert!(!body.matches(&Search { text: Some("golf".into()), ..Default::default() }));
    }

    #[test]
    fn implausible_demographics_rejected() {
        let mut d = demo(30, 180, "x", "y");
        d.age = Some(12);
        assert_eq!(d.validate(), Err(ProfileError::BadAge));
        let mut d = demo(30, 180, "x", "y");
        d.height_cm = Some(900);
        assert_eq!(d.validate(), Err(ProfileError::BadDemographic));
        let mut d = demo(30, 180, "x", "y");
        d.ethnicity = Some("z".repeat(MAX_DEMOGRAPHIC_BYTES + 1));
        assert_eq!(d.validate(), Err(ProfileError::BadDemographic));
    }

    #[test]
    fn demographics_are_covered_by_the_signature() {
        // Editing someone's stated age in transit must break verification.
        let p = ProfileParams::new(vec![7u8; ML_DSA_65_VK_BYTES]);
        let mut a = ProfileBody { sequence: 1, ..Default::default() };
        a.demographics.age = Some(30);
        let mut b = a.clone();
        b.demographics.age = Some(21);
        assert_ne!(
            a.signing_payload_current(&p).unwrap(),
            b.signing_payload_current(&p).unwrap()
        );
    }
}

#[cfg(test)]
mod position_health_tests {
    use super::*;

    fn d(pos: Position, hiv: HivStatus) -> Demographics {
        Demographics {
            age: Some(32),
            position: Some(pos),
            gender: Some(Gender::Man),
            hiv_status: Some(hiv),
            last_tested: Some((2026, 6)),
            ..Default::default()
        }
    }

    #[test]
    fn position_codes_roundtrip_and_reserve_zero() {
        for p in [
            Position::Top, Position::VerseTop, Position::Versatile,
            Position::VerseBottom, Position::Bottom, Position::Side,
        ] {
            assert_ne!(p.code(), 0, "0 is reserved for unstated");
            assert_eq!(Position::from_code(p.code()), Some(p));
        }
        assert_eq!(Position::from_code(0), None);
        assert_eq!(Position::Versatile.label(), "Versatile");
        assert_eq!(Position::VerseBottom.label(), "Verse-Bottom");
    }

    #[test]
    fn position_filter_accepts_several_at_once() {
        let versatile = d(Position::Versatile, HivStatus::Negative);
        let top = d(Position::Top, HivStatus::Negative);
        let q = Search {
            positions: vec![Position::Versatile, Position::VerseBottom],
            ..Default::default()
        };
        assert!(versatile.matches(&q));
        assert!(!top.matches(&q));
    }

    #[test]
    fn hiv_filter_matches_listed_statuses() {
        let undetectable = d(Position::Top, HivStatus::PositiveUndetectable);
        let on_prep = d(Position::Top, HivStatus::NegativeOnPrep);
        let q = Search {
            hiv_statuses: vec![HivStatus::PositiveUndetectable],
            ..Default::default()
        };
        assert!(undetectable.matches(&q));
        assert!(!on_prep.matches(&q));
    }

    #[test]
    fn unstated_position_or_status_never_matches_a_filter() {
        // Same rule as age: a filter must never silently include people
        // who declined to answer. It matters most here.
        let quiet = Demographics { age: Some(30), ..Default::default() };
        assert!(!quiet.matches(&Search { positions: vec![Position::Top], ..Default::default() }));
        assert!(!quiet.matches(&Search {
            hiv_statuses: vec![HivStatus::Negative],
            ..Default::default()
        }));
        assert!(quiet.matches(&Search::default()), "still visible unfiltered");
    }

    #[test]
    fn implausible_test_dates_rejected() {
        let mut x = d(Position::Top, HivStatus::Negative);
        x.last_tested = Some((2026, 13));
        assert_eq!(x.validate(), Err(ProfileError::BadDemographic));
        x.last_tested = Some((1900, 6));
        assert_eq!(x.validate(), Err(ProfileError::BadDemographic));
    }

    #[test]
    fn position_and_status_are_covered_by_the_signature() {
        let p = ProfileParams::new(vec![7u8; ML_DSA_65_VK_BYTES]);
        let a = ProfileBody {
            demographics: d(Position::Top, HivStatus::Negative),
            sequence: 1,
            ..Default::default()
        };
        let mut b = a.clone();
        b.demographics.hiv_status = Some(HivStatus::Positive);
        assert_ne!(
            a.signing_payload_current(&p).unwrap(),
            b.signing_payload_current(&p).unwrap(),
            "altering someone's stated status in transit must break verification"
        );
    }
}

#[cfg(test)]
mod gender_tests {
    use super::*;

    #[test]
    fn named_genders_filter() {
        let nb = Demographics { gender: Some(Gender::NonBinary), ..Default::default() };
        let tw = Demographics { gender: Some(Gender::TransWoman), ..Default::default() };
        let q = Search {
            genders: vec![Gender::NonBinary, Gender::TransMan],
            ..Default::default()
        };
        assert!(nb.matches(&q));
        assert!(!tw.matches(&q));
    }

    #[test]
    fn self_described_gender_is_supported_and_matchable() {
        // The escape hatch has to actually work, not just exist.
        let g = Gender::SelfDescribed("genderqueer".into());
        let d = Demographics { gender: Some(g.clone()), ..Default::default() };
        assert!(d.validate().is_ok());
        assert_eq!(d.gender.as_ref().unwrap().label(), "genderqueer");
        assert!(d.matches(&Search { genders: vec![g], ..Default::default() }));
    }

    #[test]
    fn empty_self_description_is_rejected() {
        let d = Demographics {
            gender: Some(Gender::SelfDescribed(String::new())),
            ..Default::default()
        };
        assert_eq!(d.validate(), Err(ProfileError::BadDemographic));
    }

    #[test]
    fn not_specified_is_searchable_on_purpose() {
        // Distinct from "any": someone who stated no position must be
        // reachable by a search that deliberately asks for them, or they
        // are invisible to every filtered search there is.
        let quiet = Demographics { age: Some(30), ..Default::default() };
        let stated = Demographics {
            age: Some(30),
            position: Some(Position::Top),
            ..Default::default()
        };
        let q = Search { include_unspecified_position: true, ..Default::default() };
        assert!(quiet.matches(&q));

        let strict = Search { positions: vec![Position::Bottom], ..Default::default() };
        assert!(!quiet.matches(&strict), "and still absent from a specific filter");
        assert!(!stated.matches(&strict));
    }

    #[test]
    fn gender_is_covered_by_the_signature() {
        let p = ProfileParams::new(vec![7u8; ML_DSA_65_VK_BYTES]);
        let a = ProfileBody {
            demographics: Demographics { gender: Some(Gender::TransMan), ..Default::default() },
            sequence: 1,
            ..Default::default()
        };
        let mut b = a.clone();
        b.demographics.gender = Some(Gender::Man);
        assert_ne!(
            a.signing_payload_current(&p).unwrap(),
            b.signing_payload_current(&p).unwrap(),
            "nobody may rewrite someone's stated gender in transit"
        );
    }
}
