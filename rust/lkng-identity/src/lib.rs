//! LKNG identity: ML-DSA-65 keys, scoped signing, and recoverable backups.
//!
//! Three jobs, in order of how much trouble each prevents:
//!
//! 1. **Sign and verify presence records** using
//!    [`lkng_presence::PresenceRecord::signing_payload`], so a signature is
//!    valid only in the cell and epoch it was minted for.
//! 2. **Hold the key material** so nothing above this crate ever sees it.
//!    In production this compiles into the identity delegate; the WebView
//!    can ask for signatures but never for the key.
//! 3. **Encrypt a recovery bundle** under a passphrase, so an account
//!    survives a lost phone without any server holding anything.
//!
//! ML-DSA-65 (FIPS 204) matches Mail and Raven. Sizes to budget for:
//! verifying key 1952 B, signature 3309 B, seed 32 B. The seed is what the
//! backup stores — 32 bytes regenerates everything.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use lkng_presence::{CellParams, PresenceRecord};
use ml_dsa::{MlDsa65, Signature, SigningKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Signing-context tag passed to ML-DSA's own context parameter. A second,
/// independent layer of domain separation beneath the one baked into the
/// signed payload — belt and braces, and free.
pub const SIGN_CONTEXT: &[u8] = b"lkng/v1";

/// Domain tag for per-epoch subkey derivation. Wire format — changing it
/// orphans every existing epoch key.
pub const EPOCH_KEY_DOMAIN: &[u8] = b"lkng/epoch-key/v1";

/// Argon2id parameters for recovery-passphrase stretching.
///
/// Deliberately expensive: the backup bundle is a public contract on the
/// network, so anyone who guesses the derivation scheme can attack it
/// offline, forever. 64 MiB / 3 passes is roughly the OWASP floor and
/// costs a phone well under a second. Weak passphrases still lose — the
/// UI must say so and enforce a strength meter.
const ARGON_MEM_KIB: u32 = 64 * 1024;
const ARGON_PASSES: u32 = 3;
const ARGON_LANES: u32 = 1;

/// Bundle wire-format version. Bump on any layout change; old versions
/// must keep decoding (see `freenet-git`'s pinned wire-format tests).
pub const BUNDLE_V1: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("signature verification failed")]
    BadSignature,
    #[error("malformed key material")]
    BadKey,
    #[error("wrong passphrase, or the bundle is corrupt")]
    Undecryptable,
    #[error("unsupported bundle version {0}")]
    UnsupportedVersion(u8),
    #[error("encode: {0}")]
    Encode(String),
    #[error("presence: {0}")]
    Presence(#[from] lkng_presence::PresenceError),
}

/// A device identity. Holds secret key material — never serialize this
/// type directly; use [`Identity::to_backup`].
pub struct Identity {
    seed: [u8; 32],
    signing: SigningKey<MlDsa65>,
}

impl Drop for Identity {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}

impl Identity {
    /// Create from a 32-byte seed. The caller supplies randomness so this
    /// crate stays agnostic about the RNG (the delegate uses the platform
    /// CSPRNG; tests use a fixed seed).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let signing = SigningKey::<MlDsa65>::from_seed(&seed.into());
        Self { seed, signing }
    }

    /// The public verifying key, encoded (1952 bytes).
    pub fn verifying_key_bytes(&self) -> Vec<u8> {
        self.signing.expanded_key().verifying_key().encode().to_vec()
    }

    /// Short shareable handle: first 8 bytes of BLAKE3(verifying key),
    /// base58. Same construction ghostkeys uses, so handles look
    /// consistent across the ecosystem.
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.verifying_key_bytes())
    }

    /// The pseudonym published in a presence record: BLAKE3 of the
    /// verifying key. Not the key itself, so a scraper harvesting a cell
    /// does not walk away with 1952-byte keys it can index.
    pub fn pseudonym(&self) -> [u8; 32] {
        *blake3::hash(&self.verifying_key_bytes()).as_bytes()
    }

    /// Derive the throwaway identity used to sign presence for one epoch.
    ///
    /// **This is what makes pseudonym rotation mean anything.** A presence
    /// record carries its verifying key inline so peers can validate it
    /// without a lookup — which means that key is public to anyone scraping
    /// a cell. If tiles were signed with the durable identity, a scraper
    /// could take the key straight from a tile, derive the owner's profile
    /// address, and pull the full profile: "revealed only after mutual
    /// interaction" would be dead on arrival, and every tile would be
    /// permanently linkable to one person across all cells and epochs.
    ///
    /// So tiles are signed by a per-epoch subkey derived here. BLAKE3's
    /// keyed hash is a PRF, so holding one epoch key tells an attacker
    /// nothing about any other epoch key or about the master seed. The
    /// owner can always regenerate any epoch's key from the master.
    ///
    /// Linking an epoch key back to a durable profile is possible only
    /// when its owner chooses to prove it, during the match handshake.
    pub fn for_epoch(&self, epoch: u64) -> Identity {
        let mut data = Vec::with_capacity(EPOCH_KEY_DOMAIN.len() + 8);
        data.extend_from_slice(EPOCH_KEY_DOMAIN);
        data.extend_from_slice(&epoch.to_le_bytes());
        let sub = blake3::keyed_hash(&self.seed, &data);
        Identity::from_seed(*sub.as_bytes())
    }

    /// Sign a presence record for a specific cell and epoch, filling in
    /// `pseudonym`, `verifying_key` and `sig`.
    ///
    /// Always signs with the **epoch subkey** taken from `params.epoch` —
    /// the durable identity key never appears in public state, and there is
    /// no API to make it do so by accident.
    pub fn sign_presence(
        &self,
        record: &mut PresenceRecord,
        params: &CellParams,
    ) -> Result<(), IdentityError> {
        self.for_epoch(params.epoch).sign_presence_raw(record, params)
    }

    /// Sign with *this* key directly. Private: the only caller is
    /// [`Identity::sign_presence`], on an already-derived epoch key.
    fn sign_presence_raw(
        &self,
        record: &mut PresenceRecord,
        params: &CellParams,
    ) -> Result<(), IdentityError> {
        record.pseudonym = self.pseudonym();
        record.verifying_key = Some(self.verifying_key_bytes());
        let payload = record.signing_payload(params)?;
        let sig: Signature<MlDsa65> = self
            .signing
            .expanded_key()
            .sign_deterministic(&payload, SIGN_CONTEXT)
            .map_err(|_| IdentityError::BadKey)?;
        record.sig = sig.encode().to_vec();
        Ok(())
    }

    /// Encrypt this identity into a recovery bundle under `passphrase`.
    pub fn to_backup(&self, passphrase: &str, salt: [u8; 16]) -> Result<Vec<u8>, IdentityError> {
        let mut key = derive_key(passphrase, &salt);
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
        // Nonce derived from the salt: each bundle has a fresh salt, so the
        // (key, nonce) pair is never reused, and one 16-byte salt beats
        // storing salt + nonce separately.
        let nonce_bytes = derive_nonce(&salt);
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce_bytes), self.seed.as_ref())
            .map_err(|_| IdentityError::BadKey)?;
        key.zeroize();

        let bundle = Bundle {
            version: BUNDLE_V1,
            salt,
            ciphertext,
            argon: ArgonParams {
                mem_kib: ARGON_MEM_KIB,
                passes: ARGON_PASSES,
                lanes: ARGON_LANES,
            },
        };
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&bundle, &mut buf)
            .map_err(|e| IdentityError::Encode(e.to_string()))?;
        Ok(buf)
    }

    /// Recover an identity from a bundle. A wrong passphrase is
    /// indistinguishable from corruption, by design.
    pub fn from_backup(bundle_bytes: &[u8], passphrase: &str) -> Result<Self, IdentityError> {
        let bundle: Bundle = ciborium::de::from_reader(bundle_bytes)
            .map_err(|e| IdentityError::Encode(e.to_string()))?;
        if bundle.version != BUNDLE_V1 {
            return Err(IdentityError::UnsupportedVersion(bundle.version));
        }
        let mut key = derive_key_with(passphrase, &bundle.salt, &bundle.argon);
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
        let nonce_bytes = derive_nonce(&bundle.salt);
        let plain = cipher
            .decrypt(XNonce::from_slice(&nonce_bytes), bundle.ciphertext.as_ref())
            .map_err(|_| IdentityError::Undecryptable)?;
        key.zeroize();

        let seed: [u8; 32] = plain
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::BadKey)?;
        Ok(Self::from_seed(seed))
    }

    /// Locator for the backup contract: derived from the passphrase alone,
    /// so a new phone can find the bundle knowing only what the user
    /// remembers.
    ///
    /// This is the whole recovery trick and also its whole risk — the
    /// address is public and guessable at exactly the cost of guessing the
    /// passphrase. Argon2id's work factor is the only thing between a weak
    /// passphrase and a stranger's decryption.
    pub fn backup_locator(passphrase: &str) -> [u8; 32] {
        let mut k = derive_key(passphrase, b"lkng/backup-locator\0");
        let out = *blake3::hash(&k).as_bytes();
        k.zeroize();
        out
    }
}

/// Verify a presence record. Re-exported from `lkng-presence` so the
/// contract (which compiles to wasm32 and cannot pull an RNG) and the
/// client share exactly one implementation — divergence between the two
/// would mean records that validate on one side and not the other.
pub use lkng_presence::verify::{verify_record as verify_presence, verify_self_contained};

/// Short handle from encoded verifying-key bytes.
pub fn fingerprint_of(verifying_key_bytes: &[u8]) -> String {
    let h = blake3::hash(verifying_key_bytes);
    bs58::encode(&h.as_bytes()[..8]).into_string()
}

#[derive(Serialize, Deserialize)]
struct ArgonParams {
    mem_kib: u32,
    passes: u32,
    lanes: u32,
}

#[derive(Serialize, Deserialize)]
struct Bundle {
    version: u8,
    salt: [u8; 16],
    #[serde(with = "serde_bytes")]
    ciphertext: Vec<u8>,
    argon: ArgonParams,
}

fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
    derive_key_with(
        passphrase,
        salt,
        &ArgonParams {
            mem_kib: ARGON_MEM_KIB,
            passes: ARGON_PASSES,
            lanes: ARGON_LANES,
        },
    )
}

/// Parameters travel *in* the bundle so a future increase in work factor
/// doesn't lock existing users out of their own backups.
fn derive_key_with(passphrase: &str, salt: &[u8], p: &ArgonParams) -> [u8; 32] {
    let params =
        Params::new(p.mem_kib, p.passes, p.lanes, Some(32)).expect("argon2 params are in range");
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .expect("argon2 derivation cannot fail with valid params");
    out
}

fn derive_nonce(salt: &[u8; 16]) -> [u8; 24] {
    let h = blake3::keyed_hash(&[7u8; 32], salt);
    let mut n = [0u8; 24];
    n.copy_from_slice(&h.as_bytes()[..24]);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(cell: &str, epoch: u64) -> CellParams {
        CellParams {
            schema_v: 1,
            cell_id: cell.into(),
            epoch,
        }
    }

    fn blank_record() -> PresenceRecord {
        PresenceRecord {
            pseudonym: [0; 32],
            headline: "looking".into(),
            thumbnail: vec![9; 128],
            timestamp_ms: 1_785_523_000_000,
            verifying_key: None,
            writer_cert: None,
            sig: vec![],
        }
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let id = Identity::from_seed([3; 32]);
        let p = params("9q8yy", 20666);
        let mut r = blank_record();
        id.sign_presence(&mut r, &p).unwrap();

        assert_eq!(r.sig.len(), 3309, "ML-DSA-65 signature size");
        assert_eq!(id.verifying_key_bytes().len(), 1952, "ML-DSA-65 vk size");
        assert!(r.validate().is_ok(), "signed record must satisfy state caps");
        // Verified against the key the record carries (the epoch subkey),
        // which is exactly what a peer on the network can do.
        verify_self_contained(&r, &p).unwrap();
    }

    #[test]
    fn signature_does_not_transfer_to_another_cell() {
        // The vulnerability this whole design exists to close.
        let id = Identity::from_seed([3; 32]);
        let mut r = blank_record();
        id.sign_presence(&mut r, &params("9q8yy", 20666)).unwrap();

        assert!(
            verify_self_contained(&r, &params("dr5ru", 20666)).is_err(),
            "record must not verify in a different cell"
        );
        assert!(
            verify_self_contained(&r, &params("9q8yy", 20667)).is_err(),
            "record must not verify in a different epoch"
        );
    }

    #[test]
    fn tampered_content_fails() {
        let id = Identity::from_seed([3; 32]);
        let p = params("9q8yy", 20666);
        let mut r = blank_record();
        id.sign_presence(&mut r, &p).unwrap();
        r.headline = "different".into();
        assert!(verify_self_contained(&r, &p).is_err());
    }

    #[test]
    fn stolen_signature_under_wrong_pseudonym_fails() {
        let alice = Identity::from_seed([1; 32]);
        let bob = Identity::from_seed([2; 32]);
        let p = params("9q8yy", 20666);
        let mut r = blank_record();
        alice.sign_presence(&mut r, &p).unwrap();
        // Bob parades Alice's signed tile under his own epoch identity.
        let bob_epoch = bob.for_epoch(p.epoch);
        r.pseudonym = bob_epoch.pseudonym();
        r.verifying_key = Some(bob_epoch.verifying_key_bytes());
        assert!(verify_self_contained(&r, &p).is_err());
    }

    #[test]
    fn presence_never_exposes_the_durable_key() {
        // The property the whole rotation scheme rests on: a scraper who
        // harvests a tile must not learn the durable identity.
        let id = Identity::from_seed([3; 32]);
        let p = params("9q8yy", 20666);
        let mut r = blank_record();
        id.sign_presence(&mut r, &p).unwrap();

        let published = r.verifying_key.clone().expect("key travels with record");
        assert_ne!(
            published,
            id.verifying_key_bytes(),
            "durable verifying key must never appear in public state"
        );
        assert_ne!(r.pseudonym, id.pseudonym());
        // ...and the tile still verifies on its own terms.
        verify_presence(&r, &p, &published).unwrap();
    }

    #[test]
    fn epoch_keys_are_unlinkable_across_epochs() {
        let id = Identity::from_seed([3; 32]);
        let a = id.for_epoch(20666).verifying_key_bytes();
        let b = id.for_epoch(20667).verifying_key_bytes();
        assert_ne!(a, b, "each epoch must present a fresh key");
        // Deterministic: the owner can re-derive to update their own tile.
        assert_eq!(a, id.for_epoch(20666).verifying_key_bytes());
    }

    #[test]
    fn different_users_derive_different_epoch_keys() {
        let a = Identity::from_seed([1; 32]).for_epoch(20666).verifying_key_bytes();
        let b = Identity::from_seed([2; 32]).for_epoch(20666).verifying_key_bytes();
        assert_ne!(a, b);
    }

    #[test]
    fn recovered_identity_regenerates_the_same_epoch_keys() {
        // Recovery must restore the ability to update tiles already posted.
        let id = Identity::from_seed([5; 32]);
        let bundle = id.to_backup("passphrase", [1; 16]).unwrap();
        let restored = Identity::from_backup(&bundle, "passphrase").unwrap();
        assert_eq!(
            id.for_epoch(20666).verifying_key_bytes(),
            restored.for_epoch(20666).verifying_key_bytes()
        );
    }

    #[test]
    fn backup_roundtrip_recovers_same_identity() {
        let id = Identity::from_seed([5; 32]);
        let bundle = id.to_backup("correct horse battery staple", [1; 16]).unwrap();
        let restored = Identity::from_backup(&bundle, "correct horse battery staple").unwrap();
        assert_eq!(id.verifying_key_bytes(), restored.verifying_key_bytes());
        assert_eq!(id.fingerprint(), restored.fingerprint());

        // A recovered identity still signs verifiably — the whole point.
        let p = params("9q8yy", 20666);
        let mut r = blank_record();
        restored.sign_presence(&mut r, &p).unwrap();
        verify_self_contained(&r, &p).unwrap();
    }

    #[test]
    fn wrong_passphrase_fails_cleanly() {
        let id = Identity::from_seed([5; 32]);
        let bundle = id.to_backup("right", [1; 16]).unwrap();
        assert!(matches!(
            Identity::from_backup(&bundle, "wrong"),
            Err(IdentityError::Undecryptable)
        ));
    }

    #[test]
    fn backup_locator_is_passphrase_derived_and_stable() {
        let a = Identity::backup_locator("shared secret");
        let b = Identity::backup_locator("shared secret");
        let c = Identity::backup_locator("other secret");
        assert_eq!(a, b, "same passphrase must find the same bundle");
        assert_ne!(a, c);
    }

    #[test]
    fn fingerprints_are_short_and_distinct() {
        let a = Identity::from_seed([1; 32]).fingerprint();
        let b = Identity::from_seed([2; 32]).fingerprint();
        assert_ne!(a, b);
        assert!(a.len() <= 12, "handle must be short enough to share: {a}");
    }
}
