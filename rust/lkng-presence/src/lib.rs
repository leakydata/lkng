//! Presence-cell state for LKNG.
//!
//! One cell contract per `(cell_id, epoch)` (those live in contract
//! *parameters*; this crate is the state that goes inside). The shape is
//! Raven's `global-index-shard`, adapted: an **anyone-writes grow-set of
//! self-contained records**, deduplicated by content id, **capped
//! post-merge** to the newest [`MAX_RECORDS`] by the total order
//! `(timestamp desc, id desc)`.
//!
//! The two Raven lessons this crate exists to encode (see PLAN.md):
//!
//! 1. **The cap is applied after merge, never used to reject state.** A
//!    transiently over-bound merged state is normal; rejecting it breaks
//!    convergence. `validate` checks per-record invariants only.
//! 2. **Truncation must itself be order-independent**, which requires a
//!    *total* order — `(timestamp, id)` with the content id as tiebreak —
//!    so that any two peers, merging any update orders, keep the same N.
//!
//! Records are **self-contained** (River #145): everything needed to render
//! a grid tile travels in the record. And per the plan's write-gating
//! reservation, [`PresenceRecord::writer_cert`] exists from v1 (always
//! `None` today) so AFT/Ghost-Key gating can arrive **without rotating the
//! contract ID**.

use std::collections::{BTreeMap, BTreeSet};

use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};

/// Domain-separation tag for presence signatures. Borrowed from the
/// ghostkey delegate's `ScopedPayload` discipline: *the raw payload is
/// never signed alone*. Changing this invalidates every existing
/// signature — it is wire format, not a constant to tidy.
pub const SIG_DOMAIN: &str = "lkng/presence-record/v1";

/// Which cell and epoch a state belongs to. These are the contract
/// *parameters* (part of `hash(code, params)`, so each `(cell, epoch)` is
/// its own contract), and they are ALSO covered by every record signature
/// — see [`PresenceRecord::signing_payload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellParams {
    pub schema_v: u8,
    /// Geohash level-5 cell id (see `lkng-location`).
    pub cell_id: String,
    /// Presence epoch index; see [`epoch_for_unix_time`].
    pub epoch: u64,
}

/// Hard cap on records retained per cell. A flood evicts older genuine
/// records (lossy, bounded blast radius) — the write-gate slot is the real
/// abuse control later.
pub const MAX_RECORDS: usize = 500;
/// Headline cap (bytes). Grindr-style short text.
pub const MAX_HEADLINE_BYTES: usize = 140;
/// Thumbnail cap (bytes). LOAD-BEARING: every phone in the cell pays
/// `records × thumbnail` in bandwidth. 16 KiB ≈ a 96×96 webp at q≈60.
pub const MAX_THUMBNAIL_BYTES: usize = 16 * 1024;
/// Signature length cap (ML-DSA-65 signatures are 3309 bytes).
pub const MAX_SIG_BYTES: usize = 4096;
/// Reserved writer-cert slot cap.
pub const MAX_WRITER_CERT_BYTES: usize = 8192;
/// Encoded ML-DSA-65 verifying key length (FIPS 204).
pub const ML_DSA_65_VK_BYTES: usize = 1952;

/// Content id: BLAKE3 of the record's canonical bytes (sans nothing — the
/// whole record, signature included, is the identity: a re-signed record is
/// a different record, which is fine for a grow-set).
pub type RecordId = [u8; 32];

/// Presence epoch length. Epoch rollover is the pruning mechanism (each
/// `(cell, epoch)` is a separate contract), so this is also the maximum
/// staleness of a "nearby" tile. 6 h balances contract churn against grid
/// freshness; clients subscribe to the current and previous epoch so a
/// rollover never empties the grid.
pub const EPOCH_SECONDS: u64 = 6 * 60 * 60;

/// Epoch index for a given unix time (seconds). Client-side only — the
/// contract never reads a clock; the epoch it belongs to is pinned in its
/// parameters.
pub fn epoch_for_unix_time(unix_secs: u64) -> u64 {
    unix_secs / EPOCH_SECONDS
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PresenceError {
    #[error("headline exceeds {MAX_HEADLINE_BYTES} bytes")]
    HeadlineTooLong,
    #[error("thumbnail exceeds {MAX_THUMBNAIL_BYTES} bytes")]
    ThumbnailTooLarge,
    #[error("signature is empty or exceeds {MAX_SIG_BYTES} bytes")]
    MalformedSignature,
    #[error("writer cert exceeds {MAX_WRITER_CERT_BYTES} bytes")]
    WriterCertTooLarge,
    #[error("verifying key is not a valid ML-DSA-65 key")]
    BadVerifyingKey,
    #[error("encode: {0}")]
    Encode(String),
    #[error("signature verification failed")]
    VerificationFailed,
}

/// One grid tile: everything a peer needs to render it, self-contained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceRecord {
    /// Per-epoch pseudonym (verifying-key hash or ephemeral key id). The
    /// durable profile is NOT referenced here — it is revealed only after
    /// mutual interaction (see PLAN.md, linkability ceiling).
    pub pseudonym: [u8; 32],
    /// Short public teaser text.
    pub headline: String,
    /// Public teaser image (already resized, EXIF-stripped, re-encoded —
    /// enforced at the delegate before signing, re-checked here by size).
    #[serde(with = "serde_bytes")]
    pub thumbnail: Vec<u8>,
    /// Coarse age band (decade; 0 = unstated) so the grid can filter
    /// without anyone publishing an exact age. See `lkng_app::TileFilters`
    /// for why this is a band and not a number.
    #[serde(default)]
    pub age_band: u8,
    /// Sexual position code (0 = unstated); see `lkng_profile::Position`.
    ///
    /// Allowed here because it is preference data, not health data, and it
    /// is the filter people use most. **Nothing clinical belongs on a
    /// tile** — a tile is public to anyone subscribing to a cell.
    #[serde(default)]
    pub position: u8,
    /// Client-claimed capture time, ms since epoch. Untrusted (any client
    /// can lie); used ONLY as the retention ordering key, where lying
    /// forward merely evicts you sooner from someone else's cap.
    pub timestamp_ms: u64,
    /// The signer's encoded ML-DSA-65 verifying key (1952 B). Carried
    /// inline so the record is **self-contained** (River #145): a peer that
    /// has never seen this pseudonym can still validate the tile without a
    /// second lookup that might silently fail.
    #[serde(default, with = "serde_bytes")]
    pub verifying_key: Option<Vec<u8>>,
    /// RESERVED write-gating slot (AFT token / Ghost Key cert). Always
    /// `None` in v1; validated for size so a future value can't bloat state.
    #[serde(default, with = "serde_bytes")]
    pub writer_cert: Option<Vec<u8>>,
    /// Signature by the pseudonym key over the canonical record-sans-sig.
    /// Verification proves WHO signed, not that the signer is ALLOWED.
    #[serde(with = "serde_bytes")]
    pub sig: Vec<u8>,
}

impl PresenceRecord {
    /// Per-record invariants — the ONLY thing state validation may check.
    pub fn validate(&self) -> Result<(), PresenceError> {
        if self.headline.len() > MAX_HEADLINE_BYTES {
            return Err(PresenceError::HeadlineTooLong);
        }
        if self.thumbnail.len() > MAX_THUMBNAIL_BYTES {
            return Err(PresenceError::ThumbnailTooLarge);
        }
        if self.sig.is_empty() || self.sig.len() > MAX_SIG_BYTES {
            return Err(PresenceError::MalformedSignature);
        }
        if let Some(cert) = &self.writer_cert {
            if cert.len() > MAX_WRITER_CERT_BYTES {
                return Err(PresenceError::WriterCertTooLarge);
            }
        }
        if let Some(vk) = &self.verifying_key {
            if vk.len() != ML_DSA_65_VK_BYTES {
                return Err(PresenceError::BadVerifyingKey);
            }
        }
        Ok(())
    }

    /// Content id = BLAKE3(canonical CBOR of the whole record).
    pub fn id(&self) -> Result<RecordId, PresenceError> {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(self, &mut buf)
            .map_err(|e| PresenceError::Encode(e.to_string()))?;
        Ok(*blake3::hash(&buf).as_bytes())
    }

    /// The bytes a signature must cover.
    ///
    /// **This is a security boundary, not a serialization detail.** The
    /// record's own fields do NOT identify which cell or epoch it belongs
    /// to — those live in the contract parameters. Signing the record
    /// alone would let anyone lift a valid record out of cell A and replay
    /// it into cell B (or a future epoch), fabricating presence anywhere
    /// on earth from one honestly-signed tile. Binding `(domain, cell_id,
    /// epoch)` into the signed payload makes a signature valid **only** in
    /// the contract it was minted for.
    ///
    /// The domain tag additionally stops a signature over some other LKNG
    /// structure (a profile update, a message) from being reinterpreted as
    /// a presence record — the cross-protocol confusion that the ghostkey
    /// delegate's `ScopedPayload` wrapper exists to prevent.
    pub fn signing_payload(&self, params: &CellParams) -> Result<Vec<u8>, PresenceError> {
        #[derive(Serialize)]
        struct Scoped<'a> {
            domain: &'a str,
            schema_v: u8,
            cell_id: &'a str,
            epoch: u64,
            pseudonym: &'a [u8; 32],
            headline: &'a str,
            thumbnail: &'a [u8],
            timestamp_ms: u64,
            age_band: u8,
            position: u8,
            writer_cert: Option<&'a [u8]>,
            verifying_key: Option<&'a [u8]>,
        }
        let scoped = Scoped {
            domain: SIG_DOMAIN,
            schema_v: params.schema_v,
            cell_id: &params.cell_id,
            epoch: params.epoch,
            pseudonym: &self.pseudonym,
            headline: &self.headline,
            thumbnail: &self.thumbnail,
            timestamp_ms: self.timestamp_ms,
            age_band: self.age_band,
            position: self.position,
            writer_cert: self.writer_cert.as_deref(),
            verifying_key: self.verifying_key.as_deref(),
        };
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&scoped, &mut buf)
            .map_err(|e| PresenceError::Encode(e.to_string()))?;
        Ok(buf)
    }
}

/// Retention key: newest first, content id as total-order tiebreak.
/// Everything about convergence hangs on this being a TOTAL order.
fn retention_key(r: &PresenceRecord, id: &RecordId) -> (u64, RecordId) {
    (r.timestamp_ms, *id)
}

/// Cell state: grow-set by content id, capped post-merge.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CellState {
    pub records: BTreeMap<RecordId, PresenceRecord>,
}

impl CellState {
    /// Validate per-record invariants. Deliberately does NOT enforce
    /// [`MAX_RECORDS`] — a transiently over-bound merged state is normal
    /// (Raven lesson #1); rejecting it would break convergence.
    pub fn validate(&self) -> Result<(), PresenceError> {
        for r in self.records.values() {
            r.validate()?;
        }
        Ok(())
    }

    /// Insert a record if valid; silently drops invalid ones (merge must
    /// not fail on a bad peer record — it just doesn't take it).
    pub fn insert(&mut self, record: PresenceRecord) {
        if record.validate().is_ok() {
            if let Ok(id) = record.id() {
                self.records.insert(id, record);
            }
        }
    }

    /// Union-merge, then truncate. Commutative, associative, idempotent —
    /// including the truncation, because the retention key is total.
    pub fn merge(&mut self, other: &CellState) {
        for r in other.records.values() {
            self.insert(r.clone());
        }
        self.truncate();
    }

    /// Summary for delta sync: the set of record ids this peer holds.
    pub fn summary(&self) -> BTreeSet<RecordId> {
        self.records.keys().copied().collect()
    }

    /// Records the peer (per its summary) is missing. `None` when nothing —
    /// the contract MUST map that to zero bytes (Delta #5072), never an
    /// encoded empty vec.
    pub fn delta_for(&self, peer_has: &BTreeSet<RecordId>) -> Option<Vec<PresenceRecord>> {
        let missing: Vec<PresenceRecord> = self
            .records
            .iter()
            .filter(|(id, _)| !peer_has.contains(*id))
            .map(|(_, r)| r.clone())
            .collect();
        if missing.is_empty() {
            None
        } else {
            Some(missing)
        }
    }

    /// Apply a delta: insert each valid record, then truncate.
    ///
    /// Named distinctly from [`ComposableState::apply_delta`] so the two
    /// never shadow each other at a call site.
    pub fn apply_records(&mut self, records: Vec<PresenceRecord>) {
        for r in records {
            self.insert(r);
        }
        self.truncate();
    }

    /// Keep the newest [`MAX_RECORDS`] by `(timestamp desc, id desc)`.
    pub fn truncate(&mut self) {
        if self.records.len() <= MAX_RECORDS {
            return;
        }
        let mut keys: Vec<(u64, RecordId)> = self
            .records
            .iter()
            .map(|(id, r)| retention_key(r, id))
            .collect();
        // Sort descending; keep the first MAX_RECORDS.
        keys.sort_unstable_by(|a, b| b.cmp(a));
        let cutoff = keys[MAX_RECORDS - 1];
        self.records
            .retain(|id, r| retention_key(r, id) >= cutoff);
    }
}

/// In-contract signature verification.
///
/// This lives here rather than in `lkng-identity` because the **contract**
/// needs it and must compile to `wasm32-unknown-unknown`: signing and key
/// generation drag in an RNG (`getrandom`), which that target rejects, but
/// *verification* needs no randomness at all. `lkng-identity` re-exports
/// these for client use.
///
/// Verifying inside the contract is what keeps garbage out of state. The
/// record cap is 500; without this, anyone could fill a cell with
/// well-formed-but-unsigned junk and evict real people from the grid.
/// Raven's index shard takes the same position — every entry self-verifies
/// on every path that can enter state.
#[cfg(feature = "verify")]
pub mod verify {
    use super::*;
    use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa65, Signature, VerifyingKey};

    /// Context tag for ML-DSA's own context parameter — a second layer of
    /// domain separation beneath [`SIG_DOMAIN`]. Must match the signer.
    pub const SIGN_CONTEXT: &[u8] = b"lkng/v1";

    /// Verify one record against `params` and an encoded verifying key.
    pub fn verify_record(
        record: &PresenceRecord,
        params: &CellParams,
        verifying_key_bytes: &[u8],
    ) -> Result<(), PresenceError> {
        // The pseudonym must be the hash of the key that signed, or a valid
        // signature could be paraded under someone else's tile identity.
        if record.pseudonym != *blake3::hash(verifying_key_bytes).as_bytes() {
            return Err(PresenceError::VerificationFailed);
        }
        let enc: &EncodedVerifyingKey<MlDsa65> = verifying_key_bytes
            .try_into()
            .map_err(|_| PresenceError::VerificationFailed)?;
        let vk = VerifyingKey::<MlDsa65>::decode(enc);

        let sig_bytes: &EncodedSignature<MlDsa65> = record
            .sig
            .as_slice()
            .try_into()
            .map_err(|_| PresenceError::VerificationFailed)?;
        let sig =
            Signature::<MlDsa65>::decode(sig_bytes).ok_or(PresenceError::VerificationFailed)?;

        let payload = record.signing_payload(params)?;
        if vk.verify_with_context(&payload, SIGN_CONTEXT, &sig) {
            Ok(())
        } else {
            Err(PresenceError::VerificationFailed)
        }
    }

    /// Verify a record that carries its own verifying key inline.
    ///
    /// A presence record is *self-contained* (River #145): a peer that has
    /// never seen this pseudonym must still be able to validate the tile,
    /// so the key travels with it.
    pub fn verify_self_contained(
        record: &PresenceRecord,
        params: &CellParams,
    ) -> Result<(), PresenceError> {
        let vk = record
            .verifying_key
            .as_deref()
            .ok_or(PresenceError::VerificationFailed)?;
        verify_record(record, params, vk)
    }
}

/// Ecosystem-standard CRDT interface ([`freenet_scaffold::ComposableState`]).
///
/// LKNG's inherent methods above came out matching this trait almost
/// field-for-field, which is a good sign — but implementing it explicitly
/// buys two real things: `CellState` can be composed into a larger parent
/// state by the `#[composable]` macro (as River composes its room state),
/// and scaffold's `convergence` harness can exercise it.
///
/// `ParentState = Self`: a presence cell is a top-level contract state with
/// no sibling fields to validate against. If a future revision nests it,
/// this is the line that changes.
impl ComposableState for CellState {
    type ParentState = Self;
    type Summary = BTreeSet<RecordId>;
    type Delta = Vec<PresenceRecord>;
    type Parameters = CellParams;

    fn verify(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Result<(), String> {
        // Per-record invariants only — deliberately NOT a MAX_RECORDS check
        // (Raven lesson: a transiently over-bound merged state is normal,
        // and rejecting it breaks convergence).
        self.validate().map_err(|e| e.to_string())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.summary()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        self.delta_for(old_summary)
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(records) = delta {
            self.apply_records(records.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rec(seed: u8, ts: u64) -> PresenceRecord {
        PresenceRecord {
            pseudonym: [seed; 32],
            headline: format!("hi from {seed}"),
            thumbnail: vec![seed; 64],
            timestamp_ms: ts,
            age_band: 3,
            position: 0,
            verifying_key: None,
            writer_cert: None,
            sig: vec![seed; 64],
        }
    }

    fn params(cell: &str, epoch: u64) -> CellParams {
        CellParams { schema_v: 1, cell_id: cell.into(), epoch }
    }

    #[test]
    fn signature_payload_binds_cell_and_epoch() {
        // THE replay test: the same record signed for one cell must not
        // produce the same signable bytes in another cell or epoch.
        let r = rec(1, 100);
        let sf = r.signing_payload(&params("9q8yy", 20666)).unwrap();
        let other_cell = r.signing_payload(&params("dr5ru", 20666)).unwrap();
        let other_epoch = r.signing_payload(&params("9q8yy", 20667)).unwrap();
        assert_ne!(sf, other_cell, "record must not be replayable into another cell");
        assert_ne!(sf, other_epoch, "record must not be replayable into another epoch");
        // Deterministic for the same inputs.
        assert_eq!(sf, r.signing_payload(&params("9q8yy", 20666)).unwrap());
    }

    #[test]
    fn signature_payload_is_domain_separated() {
        let r = rec(1, 100);
        let p = r.signing_payload(&params("9q8yy", 20666)).unwrap();
        assert!(
            p.windows(SIG_DOMAIN.len()).any(|w| w == SIG_DOMAIN.as_bytes()),
            "domain tag must be inside the signed bytes"
        );
    }

    #[test]
    fn signature_payload_excludes_sig_but_covers_content() {
        let a = rec(1, 100);
        let mut b = a.clone();
        b.sig = vec![99; 64]; // different signature, same content
        let p = params("9q8yy", 20666);
        assert_eq!(
            a.signing_payload(&p).unwrap(),
            b.signing_payload(&p).unwrap(),
            "payload must not cover the sig field itself"
        );
        let mut c = a.clone();
        c.headline = "tampered".into();
        assert_ne!(
            a.signing_payload(&p).unwrap(),
            c.signing_payload(&p).unwrap(),
            "payload must cover content"
        );
    }

    #[test]
    fn composable_state_matches_inherent_methods() {
        let p = params("9q8yy", 20666);
        let mut cell = CellState::default();
        cell.insert(rec(1, 10));
        cell.insert(rec(2, 20));
        let parent = cell.clone();

        assert_eq!(cell.summarize(&parent, &p), cell.summary());
        assert!(cell.verify(&parent, &p).is_ok());

        let mut empty = CellState::default();
        let d = cell.delta(&parent, &p, &empty.summary());
        assert!(d.is_some());
        let empty_parent = empty.clone();
        ComposableState::apply_delta(&mut empty, &empty_parent, &p, &d).unwrap();
        assert_eq!(empty, cell, "trait path reproduces the state");

        // None delta is a no-op, not an error.
        let before = empty.clone();
        let bp = before.clone();
        ComposableState::apply_delta(&mut empty, &bp, &p, &None).unwrap();
        assert_eq!(empty, before);
    }

    #[test]
    fn epochs_partition_time() {
        assert_eq!(epoch_for_unix_time(0), 0);
        assert_eq!(epoch_for_unix_time(EPOCH_SECONDS - 1), 0);
        assert_eq!(epoch_for_unix_time(EPOCH_SECONDS), 1);
        // 2026-07-31T18:00:00Z
        let e = epoch_for_unix_time(1_785_520_800);
        assert_eq!(e, epoch_for_unix_time(1_785_520_800 + EPOCH_SECONDS - 1).saturating_sub(1) + 1);
    }

    #[test]
    fn caps_enforced_per_record() {
        let mut r = rec(1, 10);
        r.headline = "x".repeat(MAX_HEADLINE_BYTES + 1);
        assert_eq!(r.validate(), Err(PresenceError::HeadlineTooLong));
        let mut r = rec(1, 10);
        r.thumbnail = vec![0; MAX_THUMBNAIL_BYTES + 1];
        assert_eq!(r.validate(), Err(PresenceError::ThumbnailTooLarge));
        let mut r = rec(1, 10);
        r.sig = vec![];
        assert_eq!(r.validate(), Err(PresenceError::MalformedSignature));
    }

    #[test]
    fn invalid_records_dropped_not_fatal() {
        let mut cell = CellState::default();
        let mut bad = rec(1, 10);
        bad.thumbnail = vec![0; MAX_THUMBNAIL_BYTES + 1];
        cell.insert(bad);
        cell.insert(rec(2, 20));
        assert_eq!(cell.records.len(), 1);
        assert!(cell.validate().is_ok());
    }

    #[test]
    fn overfull_state_validates() {
        // Raven lesson #1: validation must accept a transiently over-bound
        // state; only merge truncates.
        let mut cell = CellState::default();
        for i in 0..(MAX_RECORDS + 50) {
            let mut r = rec((i % 251) as u8, i as u64);
            r.headline = format!("n{i}");
            if let Ok(id) = r.id() {
                cell.records.insert(id, r); // bypass insert-side effects
            }
        }
        assert!(cell.records.len() > MAX_RECORDS);
        assert!(cell.validate().is_ok(), "over-bound state must validate");
        cell.truncate();
        assert_eq!(cell.records.len(), MAX_RECORDS);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// THE property: any partition of any record set, merged in any
        /// order, converges to the same final state — truncation included.
        #[test]
        fn merge_is_order_independent_with_truncation(
            seeds in proptest::collection::vec((0u8..250, 0u64..1_000_000), 1..700),
            split in 1usize..699,
        ) {
            let records: Vec<PresenceRecord> =
                seeds.iter().map(|(s, t)| {
                    let mut r = rec(*s, *t);
                    r.headline = format!("{s}-{t}"); // distinct content ids
                    r
                }).collect();
            let split = split.min(records.len());

            // Path 1: A gets [0..split], B gets [split..], A merges B.
            let mut a = CellState::default();
            for r in &records[..split] { a.insert(r.clone()); }
            a.truncate();
            let mut b = CellState::default();
            for r in &records[split..] { b.insert(r.clone()); }
            b.truncate();
            let mut path1 = a.clone();
            path1.merge(&b);

            // Path 2: reverse merge direction.
            let mut path2 = b.clone();
            path2.merge(&a);

            // Path 3: everything into one state directly, then truncate.
            let mut path3 = CellState::default();
            for r in &records { path3.insert(r.clone()); }
            path3.truncate();

            prop_assert_eq!(&path1, &path2, "merge direction must not matter");
            prop_assert_eq!(&path1, &path3, "batching must not matter");
            prop_assert!(path1.records.len() <= MAX_RECORDS);
        }

        /// Idempotence: merging a state into itself changes nothing.
        #[test]
        fn merge_idempotent(
            seeds in proptest::collection::vec((0u8..250, 0u64..1_000_000), 1..100),
        ) {
            let mut cell = CellState::default();
            for (s, t) in &seeds { cell.insert(rec(*s, *t)); }
            cell.truncate();
            let snapshot = cell.clone();
            cell.merge(&snapshot.clone());
            prop_assert_eq!(cell, snapshot);
        }
    }
}
