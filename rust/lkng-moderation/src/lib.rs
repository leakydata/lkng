//! Reports, as signed claims in feeds that users choose to trust.
//!
//! # The shape, and why it is not a ban list
//!
//! There is no authority here. A report is a **descriptor** in Atlas's
//! sense: a signed statement by one party about a subject, which anyone may
//! publish and nobody is obliged to believe. A feed is a contract that
//! accumulates them. A client subscribes to the feeds it trusts and decides
//! for itself what to do with what they say.
//!
//! That is not a softer version of moderation, it is the only version
//! available: there is no server to enforce a ban, no account to suspend,
//! and no way to make a peer drop data it wants to keep. Pretending
//! otherwise would build a feature that silently does nothing.
//!
//! What it buys, which is real: a person who harasses people across a city
//! accumulates signed reports from many separate pseudonyms, and a client
//! can act on that pattern without anyone having been appointed to judge it.
//!
//! # What a report costs the reporter, stated plainly
//!
//! Reports are signed by the reporter's **epoch** key. So they are
//! pseudonymous — a report cannot be traced to a durable identity or a
//! profile — but they are *not unlinkable*: two reports from the same person
//! within one epoch share a signing key, and anyone reading the feed can see
//! that the same pseudonym filed both.
//!
//! Within an epoch, that pseudonym is also the one on their grid tile. So
//! someone who reports a person they have just been talking to has, in
//! effect, revealed which tile filed the report to anyone watching both. For
//! most reporting this does not matter. For reporting someone dangerous, in
//! a small cell, it might.
//!
//! The real fix is Harvest's blind-signed feedback tokens (see PLAN.md),
//! which make a report unlinkable to its author by construction. That is the
//! intended end state and is not built yet. Until it is, this module is
//! honest about what it is: pseudonymous, not anonymous.
//!
//! # Why the reported subject is a pseudonym and not a person
//!
//! A report names an **epoch pseudonym**, because that is all the reporter
//! has. A pseudonym rotates every six hours, so a report against one is
//! short-lived by construction — which cuts both ways. It limits the damage
//! a false report can do, and it limits how long a true one is useful.
//! Making reports durable would require reports against durable identities,
//! which would require tiles to carry durable identities, which is precisely
//! the linkability the whole design refuses.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Domain tag for report signatures. Wire format: changing it invalidates
/// every existing report.
pub const SIG_DOMAIN: &str = "lkng/report/v1";

/// Cap on reports retained per feed. Applied **post-merge**, never used to
/// reject state — the same commutative-monoid discipline as the presence
/// cell, and for the same reason: a transiently over-bound merged state is
/// normal, and rejecting it breaks convergence.
pub const MAX_REPORTS: usize = 2000;

/// Cap on the free-text note, in bytes.
///
/// Short on purpose. A report needs to say what happened, not carry an
/// essay — and every byte is replicated to everyone subscribing to the feed.
/// It is also the only free text in the system that one person writes *about
/// another*, so a generous limit would mostly enable abuse dressed as a
/// report.
pub const MAX_NOTE_BYTES: usize = 280;

pub const MAX_SIG_BYTES: usize = 4096;
pub const ML_DSA_65_VK_BYTES: usize = 1952;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ModerationError {
    #[error("note exceeds {MAX_NOTE_BYTES} bytes")]
    NoteTooLong,
    #[error("signature is empty or exceeds {MAX_SIG_BYTES} bytes")]
    MalformedSignature,
    #[error("verifying key is not a valid ML-DSA-65 key")]
    BadVerifyingKey,
    #[error("a report may not name its own author")]
    SelfReport,
    #[error("encode: {0}")]
    Encode(String),
    #[error("signature verification failed")]
    VerificationFailed,
}

/// Why something was reported.
///
/// A closed list rather than free text alone, so that clients can act on
/// reports without parsing prose, and so the categories are the same
/// everywhere rather than whatever each reporter happened to type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Reason {
    /// Abuse, threats, harassment.
    Harassment = 1,
    /// Spam or advertising.
    Spam = 2,
    /// Someone else's photos.
    Impersonation = 3,
    /// Appears to be under 18. Listed first in the UI regardless of its
    /// numbering here: it is the one report that should never be buried in
    /// a menu.
    Underage = 4,
    /// Solicitation, scams, extortion.
    Scam = 5,
    /// Anything else; the note carries it.
    Other = 6,
}

impl Reason {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Reason::Harassment => "Abuse or harassment",
            Reason::Spam => "Spam",
            Reason::Impersonation => "Not who they say they are",
            Reason::Underage => "Appears to be under 18",
            Reason::Scam => "Scam or extortion",
            Reason::Other => "Something else",
        }
    }

    /// The order the UI should offer them in.
    pub const ORDER: [Reason; 6] = [
        Reason::Underage,
        Reason::Harassment,
        Reason::Scam,
        Reason::Impersonation,
        Reason::Spam,
        Reason::Other,
    ];
}

/// Which feed this is. Feeds are separate contracts, so subscribing to one
/// is a real choice rather than a filter over a single global list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedParams {
    pub schema_v: u8,
    /// Feed name, e.g. `"baseline"`. Part of the contract parameters, so
    /// each feed has its own contract id and its own subscribers.
    pub feed: String,
}

/// One signed report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The reported epoch pseudonym.
    pub subject: [u8; 32],
    pub reason: u8,
    /// Optional free text.
    pub note: String,
    /// Reporter-claimed time. Untrusted, and used only for retention
    /// ordering — where lying forward means being shed sooner.
    pub timestamp_ms: u64,
    /// The reporter's epoch verifying key, carried inline so the record is
    /// self-contained (River #145) and can be checked without a lookup that
    /// might silently fail.
    #[serde(default, with = "serde_bytes")]
    pub verifying_key: Option<Vec<u8>>,
    #[serde(with = "serde_bytes")]
    pub sig: Vec<u8>,
}

impl Report {
    /// Per-record invariants — the only thing state validation may check.
    pub fn validate(&self) -> Result<(), ModerationError> {
        if self.note.len() > MAX_NOTE_BYTES {
            return Err(ModerationError::NoteTooLong);
        }
        if self.sig.is_empty() || self.sig.len() > MAX_SIG_BYTES {
            return Err(ModerationError::MalformedSignature);
        }
        if let Some(vk) = &self.verifying_key {
            if vk.len() != ML_DSA_65_VK_BYTES {
                return Err(ModerationError::BadVerifyingKey);
            }
            // A report naming its own author is either a bug or an attempt to
            // launder a self-endorsement into a feed. Cheap to reject, and
            // it would otherwise be a way to pad a feed with noise that looks
            // like corroboration.
            if *blake3::hash(vk).as_bytes() == self.subject {
                return Err(ModerationError::SelfReport);
            }
        }
        Ok(())
    }

    /// The bytes signed: the report, scoped to its domain and feed.
    ///
    /// The feed name is bound in, so a report filed to one feed cannot be
    /// lifted into another and presented as having been made there. Without
    /// it, anyone could take reports from a permissive feed and replay them
    /// into a strict one.
    pub fn signing_payload(&self, params: &FeedParams) -> Result<Vec<u8>, ModerationError> {
        #[derive(Serialize)]
        struct Scoped<'a> {
            domain: &'a str,
            schema_v: u8,
            feed: &'a str,
            subject: &'a [u8; 32],
            reason: u8,
            note: &'a str,
            timestamp_ms: u64,
            verifying_key: Option<&'a [u8]>,
        }
        let scoped = Scoped {
            domain: SIG_DOMAIN,
            schema_v: params.schema_v,
            feed: &params.feed,
            subject: &self.subject,
            reason: self.reason,
            note: &self.note,
            timestamp_ms: self.timestamp_ms,
            verifying_key: self.verifying_key.as_deref(),
        };
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&scoped, &mut buf)
            .map_err(|e| ModerationError::Encode(e.to_string()))?;
        Ok(buf)
    }

    /// Content id: BLAKE3 over the canonical bytes, signature included.
    pub fn id(&self) -> Result<[u8; 32], ModerationError> {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(self, &mut buf)
            .map_err(|e| ModerationError::Encode(e.to_string()))?;
        Ok(*blake3::hash(&buf).as_bytes())
    }
}

/// A feed's state: a grow-set of reports, deduplicated by content id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FeedState {
    pub reports: BTreeMap<[u8; 32], Report>,
}

impl FeedState {
    pub fn validate(&self) -> Result<(), ModerationError> {
        for r in self.reports.values() {
            r.validate()?;
        }
        Ok(())
    }

    pub fn insert(&mut self, report: Report) {
        if let Ok(id) = report.id() {
            self.reports.insert(id, report);
        }
    }

    /// Merge another state in. Commutative and idempotent: the result does
    /// not depend on the order updates arrive, which is what makes this
    /// converge across peers at all.
    pub fn merge(&mut self, other: &FeedState) {
        for (id, r) in &other.reports {
            self.reports.entry(*id).or_insert_with(|| r.clone());
        }
    }

    /// Apply the cap. **Never** called from validation — a merged state that
    /// is transiently over the cap is normal, and rejecting it would break
    /// convergence between peers that merged in different orders.
    ///
    /// Truncation uses a *total* order `(timestamp, id)`, so any two peers
    /// keep the same set regardless of how they got there.
    pub fn trim(&mut self) {
        if self.reports.len() <= MAX_REPORTS {
            return;
        }
        let mut keys: Vec<([u8; 32], u64, [u8; 32])> = self
            .reports
            .iter()
            .map(|(id, r)| (*id, r.timestamp_ms, *id))
            .collect();
        // Newest first, content id as the tiebreak that makes it total.
        keys.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
        let keep: BTreeSet<[u8; 32]> =
            keys.into_iter().take(MAX_REPORTS).map(|(id, _, _)| id).collect();
        self.reports.retain(|id, _| keep.contains(id));
    }

    /// How many distinct reporters have named this subject.
    ///
    /// Distinct *reporters*, not reports: one person filing five times is
    /// one person with a grievance, and counting it as five is how a single
    /// determined user manufactures the appearance of consensus. This is the
    /// number a client should act on.
    pub fn reporter_count(&self, subject: &[u8; 32]) -> usize {
        self.reports
            .values()
            .filter(|r| &r.subject == subject)
            .filter_map(|r| r.verifying_key.as_ref())
            .map(|vk| *blake3::hash(vk).as_bytes())
            .collect::<BTreeSet<[u8; 32]>>()
            .len()
    }
}

#[cfg(feature = "verify")]
pub mod verify {
    use super::*;
    use ml_dsa::{MlDsa65, Signature, VerifyingKey};

    pub const SIGN_CONTEXT: &[u8] = b"lkng/v1";

    /// Verify a self-contained report against its feed.
    pub fn verify_report(r: &Report, params: &FeedParams) -> Result<(), ModerationError> {
        r.validate()?;
        let vk_bytes = r
            .verifying_key
            .as_ref()
            .ok_or(ModerationError::BadVerifyingKey)?;
        let encoded: &[u8; ML_DSA_65_VK_BYTES] = vk_bytes[..]
            .try_into()
            .map_err(|_| ModerationError::BadVerifyingKey)?;
        let vk = VerifyingKey::<MlDsa65>::decode(encoded.into());
        let sig_bytes: &[u8; 3309] = r.sig[..]
            .try_into()
            .map_err(|_| ModerationError::MalformedSignature)?;
        let sig = Signature::<MlDsa65>::decode(sig_bytes.into())
            .ok_or(ModerationError::MalformedSignature)?;
        let payload = r.signing_payload(params)?;
        if vk.verify_with_context(&payload, SIGN_CONTEXT, &sig) {
            Ok(())
        } else {
            Err(ModerationError::VerificationFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> FeedParams {
        FeedParams { schema_v: 1, feed: "baseline".into() }
    }

    fn report(subject: u8, ts: u64, note: &str) -> Report {
        Report {
            subject: [subject; 32],
            reason: Reason::Spam.code(),
            note: note.into(),
            timestamp_ms: ts,
            verifying_key: None,
            sig: vec![1; 64],
        }
    }

    #[test]
    fn merging_is_commutative() {
        let (a, b) = (report(1, 10, "a"), report(2, 20, "b"));
        let mut x = FeedState::default();
        x.insert(a.clone());
        let mut y = FeedState::default();
        y.insert(b.clone());

        let mut ab = x.clone();
        ab.merge(&y);
        let mut ba = y.clone();
        ba.merge(&x);
        assert_eq!(ab, ba, "merge order must not change the result");
    }

    #[test]
    fn merging_is_idempotent() {
        let mut a = FeedState::default();
        a.insert(report(1, 10, "a"));
        let before = a.clone();
        a.merge(&before);
        assert_eq!(a, before);
    }

    /// The cap must not be enforced during validation.
    ///
    /// A merged state that is briefly over the cap is normal on a network
    /// where updates arrive in any order. Rejecting it would make peers
    /// permanently disagree — the exact failure Raven documents.
    #[test]
    fn validation_ignores_the_cap() {
        let mut s = FeedState::default();
        for i in 0..(MAX_REPORTS + 50) {
            s.insert(report(1, i as u64, &format!("n{i}")));
        }
        assert!(s.reports.len() > MAX_REPORTS);
        assert!(s.validate().is_ok(), "an over-cap state is valid, just untrimmed");
        s.trim();
        assert_eq!(s.reports.len(), MAX_REPORTS);
    }

    /// Trimming must be order-independent, or peers keep different subsets
    /// and never converge.
    #[test]
    fn trimming_is_order_independent() {
        let mut forward = FeedState::default();
        for i in 0..(MAX_REPORTS + 100) {
            forward.insert(report(1, i as u64, &format!("n{i}")));
        }
        let mut backward = FeedState::default();
        for i in (0..(MAX_REPORTS + 100)).rev() {
            backward.insert(report(1, i as u64, &format!("n{i}")));
        }
        forward.trim();
        backward.trim();
        assert_eq!(forward, backward);
    }

    #[test]
    fn a_long_note_is_rejected() {
        let r = report(1, 1, &"x".repeat(MAX_NOTE_BYTES + 1));
        assert_eq!(r.validate(), Err(ModerationError::NoteTooLong));
    }

    /// One person filing repeatedly is one reporter.
    ///
    /// Counting reports rather than reporters is how a single determined
    /// user manufactures what looks like a consensus against someone.
    #[test]
    fn one_reporter_filing_five_times_counts_once() {
        let mut s = FeedState::default();
        let vk = vec![9u8; ML_DSA_65_VK_BYTES];
        for i in 0..5 {
            let mut r = report(1, i, &format!("again {i}"));
            r.verifying_key = Some(vk.clone());
            s.insert(r);
        }
        assert_eq!(s.reports.len(), 5, "all five are stored");
        assert_eq!(s.reporter_count(&[1; 32]), 1, "but they are one reporter");
    }

    #[test]
    fn distinct_reporters_are_counted_separately() {
        let mut s = FeedState::default();
        for who in 0..3u8 {
            let mut r = report(1, who as u64, "bad");
            r.verifying_key = Some(vec![who; ML_DSA_65_VK_BYTES]);
            s.insert(r);
        }
        assert_eq!(s.reporter_count(&[1; 32]), 3);
    }

    /// A feed name is bound into the signature, so reports cannot be lifted
    /// from a permissive feed and replayed into a strict one.
    #[test]
    fn the_feed_name_is_part_of_the_signed_payload() {
        let r = report(1, 5, "x");
        let a = r.signing_payload(&params()).unwrap();
        let b = r
            .signing_payload(&FeedParams { schema_v: 1, feed: "strict".into() })
            .unwrap();
        assert_ne!(a, b, "the same report must not verify in another feed");
    }

    #[test]
    fn a_report_may_not_name_its_own_author() {
        let vk = vec![7u8; ML_DSA_65_VK_BYTES];
        let mut r = report(0, 1, "me");
        r.subject = *blake3::hash(&vk).as_bytes();
        r.verifying_key = Some(vk);
        assert_eq!(r.validate(), Err(ModerationError::SelfReport));
    }
}
