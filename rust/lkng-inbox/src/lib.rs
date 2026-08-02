//! Message requests — the "say hello" path.
//!
//! Shape taken from Mail's `contracts/inbox`: **anyone may append**, only
//! the owner may mark processed. That asymmetry is the whole design. A
//! stranger must be able to reach you without permission (otherwise there
//! is no first message), while nobody but you can alter what is already
//! there.
//!
//! ## What an inbox deliberately does not reveal
//!
//! An envelope carries no plaintext, and **no durable sender identity**.
//! The sender signs with the same per-epoch subkey that signed their tile,
//! so the recipient can prove "this is the person whose tile I tapped" —
//! and an observer of the contract learns only that *someone* wrote to
//! this inbox, which they could see anyway from the state size.
//!
//! ## Bounded, because anyone can write
//!
//! Every collection here is capped. An open inbox with an unbounded
//! collection is a free denial-of-service against the recipient's
//! bandwidth: everyone subscribed re-downloads whatever a flooder writes.
//! The caps are the reason a flood costs the flooder more than the victim.

use std::collections::{BTreeMap, BTreeSet};

use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};

/// Domain tag — distinct from presence and profile, so no signature can
/// ever be reinterpreted across the three.
pub const SIG_DOMAIN: &str = "lkng/inbox-envelope/v1";

/// Max envelopes retained. Oldest are shed first once over.
pub const MAX_ENVELOPES: usize = 256;
/// Max ciphertext per envelope. A first message is a sentence, not a file.
pub const MAX_CIPHERTEXT_BYTES: usize = 4 * 1024;
pub const MAX_SIG_BYTES: usize = 4096;
pub const ML_DSA_65_VK_BYTES: usize = 1952;
/// Cap on tombstones, so processing history cannot grow without bound.
pub const MAX_PROCESSED: usize = 1024;

pub type EnvelopeId = [u8; 32];

/// Address length — see `lkng_profile::ADDRESS_BYTES` for why this is a
/// security parameter rather than a display choice.
pub const ADDRESS_BYTES: usize = 16;

/// Address of an identity: truncated BLAKE3 of its long-term signing key.
pub fn address_of(vk: &[u8]) -> [u8; ADDRESS_BYTES] {
    blake3::hash(vk).as_bytes()[..ADDRESS_BYTES]
        .try_into()
        .expect("slice length matches ADDRESS_BYTES")
}

/// Contract parameters: *just the address* of whoever owns this inbox.
///
/// The recipient's 1952-byte verifying key lives in state, not here —
/// parameters are carried by every client on every operation, so key
/// material does not belong in them. `verify_state` rejects any state
/// whose key does not hash to this address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxParams {
    pub schema_v: u8,
    pub address: [u8; ADDRESS_BYTES],
}

impl InboxParams {
    pub fn new(recipient_vk: impl AsRef<[u8]>) -> Self {
        Self { schema_v: 1, address: address_of(recipient_vk.as_ref()) }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum InboxError {
    #[error("ciphertext is empty or exceeds {MAX_CIPHERTEXT_BYTES} bytes")]
    BadCiphertext,
    #[error("signature is empty or exceeds {MAX_SIG_BYTES} bytes")]
    MalformedSignature,
    #[error("sender key is not a valid ML-DSA-65 key")]
    BadSenderKey,
    #[error("signature verification failed")]
    VerificationFailed,
    #[error("recipient signature on the processed-set is invalid")]
    BadProcessedProof,
    #[error("encode: {0}")]
    Encode(String),
}

/// One message request. Contents are opaque to everyone but the recipient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Sender's **epoch** verifying key — the same one that signed their
    /// tile, so the recipient can tie this message to a face in the grid
    /// without either party revealing a durable identity.
    #[serde(with = "serde_bytes")]
    pub sender_epoch_vk: Vec<u8>,
    /// Which epoch that key belongs to, so the recipient knows which of
    /// their cached tiles to match it against.
    pub epoch: u64,
    /// Encrypted to the recipient. Never plaintext, at any size.
    #[serde(with = "serde_bytes")]
    pub ciphertext: Vec<u8>,
    /// Sender-claimed time. Untrusted; used only for retention ordering,
    /// where lying forward just means being shed sooner.
    pub sent_ms: u64,
    #[serde(with = "serde_bytes")]
    pub sig: Vec<u8>,
}

impl Envelope {
    pub fn validate(&self) -> Result<(), InboxError> {
        if self.ciphertext.is_empty() || self.ciphertext.len() > MAX_CIPHERTEXT_BYTES {
            return Err(InboxError::BadCiphertext);
        }
        if self.sig.is_empty() || self.sig.len() > MAX_SIG_BYTES {
            return Err(InboxError::MalformedSignature);
        }
        if self.sender_epoch_vk.len() != ML_DSA_65_VK_BYTES {
            return Err(InboxError::BadSenderKey);
        }
        Ok(())
    }

    /// Bytes covered by the signature.
    ///
    /// Binds the **recipient** as well as the contents: without that, an
    /// envelope written to one inbox could be lifted and replayed into
    /// another, letting anyone forge "this person messaged you" against a
    /// stranger. Same failure the presence records had, same fix.
    pub fn signing_payload(&self, params: &InboxParams) -> Result<Vec<u8>, InboxError> {
        #[derive(Serialize)]
        struct Scoped<'a> {
            domain: &'a str,
            schema_v: u8,
            address: &'a [u8],
            sender_epoch_vk: &'a [u8],
            epoch: u64,
            ciphertext: &'a [u8],
            sent_ms: u64,
        }
        let mut buf = Vec::new();
        ciborium::ser::into_writer(
            &Scoped {
                domain: SIG_DOMAIN,
                schema_v: params.schema_v,
                address: &params.address,
                sender_epoch_vk: &self.sender_epoch_vk,
                epoch: self.epoch,
                ciphertext: &self.ciphertext,
                sent_ms: self.sent_ms,
            },
            &mut buf,
        )
        .map_err(|e| InboxError::Encode(e.to_string()))?;
        Ok(buf)
    }

    pub fn id(&self) -> Result<EnvelopeId, InboxError> {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(self, &mut buf)
            .map_err(|e| InboxError::Encode(e.to_string()))?;
        Ok(*blake3::hash(&buf).as_bytes())
    }
}

/// The recipient's claim about which envelopes they've dealt with.
///
/// Signed, because it is destructive: an unauthenticated processed-set
/// would let any peer mark a stranger's inbox read and hide messages from
/// them. Delta's tombstone lesson, applied to a different structure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessedSet {
    pub ids: BTreeSet<EnvelopeId>,
    #[serde(default, with = "serde_bytes")]
    pub sig: Option<Vec<u8>>,
}

impl ProcessedSet {
    pub fn signing_payload(&self, params: &InboxParams) -> Result<Vec<u8>, InboxError> {
        #[derive(Serialize)]
        struct Scoped<'a> {
            domain: &'a str,
            address: &'a [u8],
            ids: &'a BTreeSet<EnvelopeId>,
        }
        let mut buf = Vec::new();
        ciborium::ser::into_writer(
            &Scoped {
                domain: "lkng/inbox-processed/v1",
                address: &params.address,
                ids: &self.ids,
            },
            &mut buf,
        )
        .map_err(|e| InboxError::Encode(e.to_string()))?;
        Ok(buf)
    }
}

/// Inbox contract state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxState {
    /// The recipient's long-term signing key, which the address commits
    /// to. In state, never in parameters.
    #[serde(default, with = "serde_bytes")]
    pub recipient_vk: Vec<u8>,
    pub envelopes: BTreeMap<EnvelopeId, Envelope>,
    pub processed: ProcessedSet,
}

impl InboxState {
    /// Per-envelope invariants only. As everywhere else, the retention cap
    /// is applied *after* merge and never used to reject state — a
    /// transiently over-bound merge is normal and rejecting it breaks
    /// convergence.
    pub fn validate(&self) -> Result<(), InboxError> {
        for e in self.envelopes.values() {
            e.validate()?;
        }
        Ok(())
    }

    pub fn insert(&mut self, env: Envelope) {
        if env.validate().is_ok() {
            if let Ok(id) = env.id() {
                self.envelopes.insert(id, env);
            }
        }
    }

    /// Union of envelopes, union of processed ids, then trim.
    ///
    /// Processed ids union rather than last-writer-wins: marking something
    /// processed is monotonic, so two devices that read different messages
    /// converge to "both read" rather than one erasing the other.
    pub fn merge(&mut self, other: &InboxState) {
        // Adopt the owner key if we do not have it yet. It is bound to the
        // address by `verify_state`, so this cannot import a stranger's.
        if self.recipient_vk.is_empty() && !other.recipient_vk.is_empty() {
            self.recipient_vk = other.recipient_vk.clone();
        }
        for e in other.envelopes.values() {
            self.insert(e.clone());
        }
        if !other.processed.ids.is_empty() {
            self.processed.ids.extend(other.processed.ids.iter().copied());
            // Keep whichever signature covers the larger set; a signature
            // over a subset no longer matches the merged ids and would be
            // rejected on the next verify.
            if other.processed.ids.len() >= self.processed.ids.len() {
                self.processed.sig = other.processed.sig.clone();
            }
        }
        self.trim();
    }

    /// Drop processed envelopes first, then the oldest, down to the cap.
    /// A total order `(sent_ms, id)` keeps this convergent.
    pub fn trim(&mut self) {
        // Processed envelopes have served their purpose; they go first.
        let processed: Vec<EnvelopeId> = self
            .envelopes
            .keys()
            .filter(|id| self.processed.ids.contains(*id))
            .copied()
            .collect();
        for id in processed {
            if self.envelopes.len() <= MAX_ENVELOPES {
                break;
            }
            self.envelopes.remove(&id);
        }
        if self.envelopes.len() > MAX_ENVELOPES {
            let mut keys: Vec<(u64, EnvelopeId)> = self
                .envelopes
                .iter()
                .map(|(id, e)| (e.sent_ms, *id))
                .collect();
            keys.sort_unstable_by(|a, b| b.cmp(a));
            let keep: BTreeSet<EnvelopeId> =
                keys.into_iter().take(MAX_ENVELOPES).map(|(_, id)| id).collect();
            self.envelopes.retain(|id, _| keep.contains(id));
        }
        if self.processed.ids.len() > MAX_PROCESSED {
            let excess = self.processed.ids.len() - MAX_PROCESSED;
            let drop: Vec<EnvelopeId> =
                self.processed.ids.iter().take(excess).copied().collect();
            for id in drop {
                self.processed.ids.remove(&id);
            }
        }
    }

    /// Envelopes still awaiting the owner's attention, newest first.
    pub fn pending(&self) -> Vec<&Envelope> {
        let mut v: Vec<(&EnvelopeId, &Envelope)> = self
            .envelopes
            .iter()
            .filter(|(id, _)| !self.processed.ids.contains(*id))
            .collect();
        v.sort_by(|a, b| b.1.sent_ms.cmp(&a.1.sent_ms).then(b.0.cmp(a.0)));
        v.into_iter().map(|(_, e)| e).collect()
    }

    pub fn summary(&self) -> InboxSummary {
        InboxSummary {
            envelope_ids: self.envelopes.keys().copied().collect(),
            processed_ids: self.processed.ids.clone(),
        }
    }

    pub fn delta_for(&self, peer: &InboxSummary) -> Option<InboxState> {
        let missing: BTreeMap<EnvelopeId, Envelope> = self
            .envelopes
            .iter()
            .filter(|(id, _)| !peer.envelope_ids.contains(*id))
            .map(|(id, e)| (*id, e.clone()))
            .collect();
        let new_processed: BTreeSet<EnvelopeId> = self
            .processed
            .ids
            .difference(&peer.processed_ids)
            .copied()
            .collect();
        if missing.is_empty() && new_processed.is_empty() {
            None
        } else {
            Some(InboxState {
                recipient_vk: self.recipient_vk.clone(),
                envelopes: missing,
                processed: ProcessedSet {
                    ids: if new_processed.is_empty() {
                        BTreeSet::new()
                    } else {
                        self.processed.ids.clone()
                    },
                    sig: self.processed.sig.clone(),
                },
            })
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxSummary {
    pub envelope_ids: BTreeSet<EnvelopeId>,
    pub processed_ids: BTreeSet<EnvelopeId>,
}

/// WASM-safe verification, shared by the contract and the client.
#[cfg(feature = "verify")]
pub mod verify {
    use super::*;
    use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa65, Signature, VerifyingKey};

    pub const SIGN_CONTEXT: &[u8] = b"lkng/v1";

    fn check(vk: &[u8], payload: &[u8], sig: &[u8]) -> Result<bool, InboxError> {
        let enc: &EncodedVerifyingKey<MlDsa65> =
            vk.try_into().map_err(|_| InboxError::BadSenderKey)?;
        let vk = VerifyingKey::<MlDsa65>::decode(enc);
        let sb: &EncodedSignature<MlDsa65> = sig
            .try_into()
            .map_err(|_| InboxError::VerificationFailed)?;
        let s = Signature::<MlDsa65>::decode(sb).ok_or(InboxError::VerificationFailed)?;
        Ok(vk.verify_with_context(payload, SIGN_CONTEXT, &s))
    }

    /// Verify one envelope against the inbox it claims to belong to.
    pub fn verify_envelope(env: &Envelope, params: &InboxParams) -> Result<(), InboxError> {
        env.validate()?;
        let payload = env.signing_payload(params)?;
        if check(&env.sender_epoch_vk, &payload, &env.sig)? {
            Ok(())
        } else {
            Err(InboxError::VerificationFailed)
        }
    }

    /// Verify a whole state: every envelope, plus the recipient's
    /// signature over the processed-set if one is present.
    pub fn verify_state(state: &InboxState, params: &InboxParams) -> Result<(), InboxError> {
        // An inbox with a processed-set must prove whose it is; an empty
        // one need not have been claimed yet.
        if !state.recipient_vk.is_empty() && address_of(&state.recipient_vk) != params.address {
            return Err(InboxError::BadProcessedProof);
        }
        for env in state.envelopes.values() {
            verify_envelope(env, params)?;
        }
        if !state.processed.ids.is_empty() {
            let sig = state
                .processed
                .sig
                .as_deref()
                .ok_or(InboxError::BadProcessedProof)?;
            let payload = state.processed.signing_payload(params)?;
            if !check(&state.recipient_vk, &payload, sig)? {
                return Err(InboxError::BadProcessedProof);
            }
        }
        Ok(())
    }
}

impl ComposableState for InboxState {
    type ParentState = Self;
    type Summary = InboxSummary;
    type Delta = InboxState;
    type Parameters = InboxParams;

    fn verify(&self, _p: &Self::ParentState, _x: &Self::Parameters) -> Result<(), String> {
        self.validate().map_err(|e| e.to_string())
    }
    fn summarize(&self, _p: &Self::ParentState, _x: &Self::Parameters) -> Self::Summary {
        InboxState::summary(self)
    }
    fn delta(
        &self,
        _p: &Self::ParentState,
        _x: &Self::Parameters,
        old: &Self::Summary,
    ) -> Option<Self::Delta> {
        self.delta_for(old)
    }
    fn apply_delta(
        &mut self,
        _p: &Self::ParentState,
        _x: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(d) = delta {
            InboxState::merge(self, d);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> InboxParams {
        InboxParams::new(vec![3u8; ML_DSA_65_VK_BYTES])
    }

    fn env(seed: u8, ts: u64) -> Envelope {
        Envelope {
            sender_epoch_vk: vec![seed; ML_DSA_65_VK_BYTES],
            epoch: 20670,
            ciphertext: vec![seed; 64],
            sent_ms: ts,
            sig: vec![seed; 64],
        }
    }

    fn state(list: Vec<Envelope>) -> InboxState {
        let mut s = InboxState::default();
        for e in list {
            s.insert(e);
        }
        s
    }

    #[test]
    fn envelope_binds_its_recipient() {
        // Without this an envelope could be lifted from one inbox and
        // replayed into another, forging "they messaged you".
        let e = env(1, 100);
        let mut other = params();
        other.address = address_of(&vec![9u8; ML_DSA_65_VK_BYTES]);
        assert_ne!(
            e.signing_payload(&params()).unwrap(),
            e.signing_payload(&other).unwrap()
        );
    }

    #[test]
    fn caps_enforced() {
        let mut e = env(1, 1);
        e.ciphertext = vec![];
        assert_eq!(e.validate(), Err(InboxError::BadCiphertext));
        let mut e = env(1, 1);
        e.ciphertext = vec![0; MAX_CIPHERTEXT_BYTES + 1];
        assert_eq!(e.validate(), Err(InboxError::BadCiphertext));
        let mut e = env(1, 1);
        e.sender_epoch_vk = vec![0; 10];
        assert_eq!(e.validate(), Err(InboxError::BadSenderKey));
    }

    #[test]
    fn merge_is_order_independent_and_idempotent() {
        let a = state(vec![env(1, 10), env(2, 20)]);
        let b = state(vec![env(2, 20), env(3, 30)]);
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba);
        let snapshot = ab.clone();
        ab.merge(&snapshot.clone());
        assert_eq!(ab, snapshot, "merging twice changes nothing");
    }

    #[test]
    fn processed_ids_union_rather_than_overwrite() {
        // Two devices read different messages; neither may erase the other.
        let mut phone = state(vec![env(1, 10), env(2, 20)]);
        let id1 = env(1, 10).id().unwrap();
        let id2 = env(2, 20).id().unwrap();
        phone.processed.ids.insert(id1);

        let mut laptop = state(vec![env(1, 10), env(2, 20)]);
        laptop.processed.ids.insert(id2);

        phone.merge(&laptop);
        assert!(phone.processed.ids.contains(&id1));
        assert!(phone.processed.ids.contains(&id2));
    }

    #[test]
    fn pending_excludes_processed_and_sorts_newest_first() {
        let mut s = state(vec![env(1, 10), env(2, 30), env(3, 20)]);
        s.processed.ids.insert(env(3, 20).id().unwrap());
        let p = s.pending();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].sent_ms, 30, "newest first");
    }

    #[test]
    fn overfull_state_still_validates() {
        // The cap must never be a validation rule (convergence).
        let mut s = InboxState::default();
        for i in 0..(MAX_ENVELOPES + 20) {
            s.insert(env((i % 250) as u8, i as u64));
        }
        assert!(s.envelopes.len() > MAX_ENVELOPES);
        assert!(s.validate().is_ok());
        s.trim();
        assert_eq!(s.envelopes.len(), MAX_ENVELOPES);
    }

    #[test]
    fn trim_sheds_processed_before_unread() {
        let mut s = InboxState::default();
        for i in 0..(MAX_ENVELOPES + 5) {
            s.insert(env((i % 250) as u8, i as u64));
        }
        // Mark the five OLDEST as processed; they should be the casualties.
        let victims: Vec<EnvelopeId> = {
            let mut v: Vec<(u64, EnvelopeId)> =
                s.envelopes.iter().map(|(id, e)| (e.sent_ms, *id)).collect();
            v.sort_unstable();
            v.into_iter().take(5).map(|(_, id)| id).collect()
        };
        for id in &victims {
            s.processed.ids.insert(*id);
        }
        s.trim();
        assert_eq!(s.envelopes.len(), MAX_ENVELOPES);
        for id in &victims {
            assert!(!s.envelopes.contains_key(id), "processed shed first");
        }
    }

    #[test]
    fn delta_is_none_when_peer_is_current() {
        let s = state(vec![env(1, 10)]);
        assert!(s.delta_for(&s.summary()).is_none());
    }

    #[test]
    fn delta_carries_only_what_is_missing() {
        let a = state(vec![env(1, 10), env(2, 20)]);
        let b = state(vec![env(1, 10)]);
        let d = a.delta_for(&b.summary()).unwrap();
        assert_eq!(d.envelopes.len(), 1);
    }
}

/// Fold a retired inbox's contents into a local view of the current one.
///
/// # This result must never be published
///
/// An envelope's signature covers the **inbox parameters** it was addressed
/// to (see [`Envelope::signing_payload`]). So mail sealed to a retired
/// address does not verify at the new one, and pushing a merged state back
/// to the network is rejected by the contract:
///
/// ```text
/// UPDATE failed: inbox failed verification: signature verification failed
/// ```
///
/// That is not a limitation to work around — it is the binding that stops
/// anyone lifting an envelope out of one person's inbox and replaying it
/// into another's. Re-signing is impossible by design: we are the
/// recipient, not the sender.
///
/// # So what migration actually is
///
/// Reading, not moving. A client fetches the retired address, decrypts what
/// it can, and keeps the plaintext in **local storage** — which is already
/// where the user's own sent messages live, and already authoritative for
/// history. The network is delivery; the device is the archive.
///
/// This function is the merge step of that read. It returns a state to
/// *display from*, never one to publish.
///
/// An earlier version of this comment asserted the opposite — that
/// envelopes are bound to the recipient's key rather than the contract, so
/// migrated state would republish cleanly. That was wrong, and no unit test
/// could have caught it: in-process, nothing checks the parameters. The
/// network did, immediately.
#[must_use = "the merged state is for display only and must not be published"]
pub fn carry_forward(current: &mut InboxState, legacy: &InboxState) -> usize {
    let before = current.envelopes.len();
    current.merge(legacy);
    current.envelopes.len() - before
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    fn envelope(n: u8, sent: u64) -> Envelope {
        Envelope {
            sender_epoch_vk: vec![n; ML_DSA_65_VK_BYTES],
            epoch: 1,
            ciphertext: vec![n; 32],
            sent_ms: sent,
            sig: vec![n; 64],
        }
    }

    #[test]
    fn carrying_forward_keeps_both_sides() {
        let mut current = InboxState::default();
        current.insert(envelope(1, 100));

        let mut legacy = InboxState::default();
        legacy.insert(envelope(2, 50));

        let added = carry_forward(&mut current, &legacy);
        assert_eq!(added, 1, "the legacy message should be carried over");
        assert_eq!(current.envelopes.len(), 2, "and the new one kept");
    }

    /// Migration must be safe to run repeatedly.
    ///
    /// It runs on startup, and startup happens constantly. If a second run
    /// duplicated messages, every relaunch would inflate the inbox until it
    /// hit the cap and started evicting real mail.
    #[test]
    fn carrying_forward_twice_changes_nothing() {
        let mut current = InboxState::default();
        let mut legacy = InboxState::default();
        legacy.insert(envelope(3, 10));

        carry_forward(&mut current, &legacy);
        let after_first = current.clone();
        let added = carry_forward(&mut current, &legacy);

        assert_eq!(added, 0, "a second migration must add nothing");
        assert_eq!(current, after_first);
    }

    /// A message already at the new address must survive migration.
    ///
    /// The failure this guards against is a migration written as a copy
    /// rather than a merge: it would silently discard anything that arrived
    /// at the new address first, which is precisely the mail received
    /// *since* the upgrade.
    #[test]
    fn migration_never_discards_newer_mail() {
        let mut current = InboxState::default();
        current.insert(envelope(9, 999));
        let expected = current.envelopes.clone();

        let mut legacy = InboxState::default();
        for i in 0..5 {
            legacy.insert(envelope(i, i as u64));
        }
        carry_forward(&mut current, &legacy);

        for (id, env) in expected {
            assert_eq!(
                current.envelopes.get(&id),
                Some(&env),
                "mail received at the new address was lost in migration"
            );
        }
    }
}
