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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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

/// Content id: BLAKE3 of the record's canonical bytes (sans nothing — the
/// whole record, signature included, is the identity: a re-signed record is
/// a different record, which is fine for a grow-set).
pub type RecordId = [u8; 32];

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PresenceError {
    #[error("headline exceeds {MAX_HEADLINE_BYTES} bytes")]
    HeadlineTooLong,
    #[error("thumbnail exceeds {MAX_THUMBNAIL_BYTES} bytes")]
    ThumbnailTooLarge,
    #[error("signature exceeds {MAX_SIG_BYTES} bytes or is empty")]
    BadSignature,
    #[error("writer cert exceeds {MAX_WRITER_CERT_BYTES} bytes")]
    WriterCertTooLarge,
    #[error("encode: {0}")]
    Encode(String),
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
    /// Client-claimed capture time, ms since epoch. Untrusted (any client
    /// can lie); used ONLY as the retention ordering key, where lying
    /// forward merely evicts you sooner from someone else's cap.
    pub timestamp_ms: u64,
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
            return Err(PresenceError::BadSignature);
        }
        if let Some(cert) = &self.writer_cert {
            if cert.len() > MAX_WRITER_CERT_BYTES {
                return Err(PresenceError::WriterCertTooLarge);
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
            writer_cert: None,
            sig: vec![seed; 64],
        }
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
        assert_eq!(r.validate(), Err(PresenceError::BadSignature));
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
