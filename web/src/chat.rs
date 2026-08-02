//! Messaging: reading your inbox contract, and sealing a first message.
//!
//! # What is on the network, and what is not
//!
//! Your inbox is a contract addressed by *your* identity. Anyone can write
//! an envelope into it — that is what makes an unsolicited first message
//! possible at all, and it is why the contract enforces caps rather than
//! permissions. Every envelope is ECIES-sealed to you, so the network
//! carries ciphertext and nothing else: the people replicating your inbox
//! learn that you receive mail, and its size, and nothing about who from.
//!
//! # Why sent messages are not on the network
//!
//! There is no "sent" contract, and there will not be one. An envelope
//! goes into the *recipient's* inbox; keeping a second copy in a contract
//! of your own would publish a list of everyone you have ever messaged,
//! indexed by your address, to anyone replicating it. Grindr keeps that
//! list on a server; here it stays in device storage or nowhere.
//!
//! That has an honest cost: reinstall without a backup and your side of
//! every conversation is gone. This is the same trade the rest of the app
//! makes, and it is the right one — but it is a real loss, not a detail.

use lkng_app::Tile;
use lkng_identity::Identity;
use lkng_inbox::{Envelope, InboxParams, InboxState};

/// What a sealed payload turned out to be.
///
/// # Why taps travel as ordinary envelopes
///
/// A tap is the cheapest possible signal — "I noticed you" — and on a
/// centralised app it is a row in a table the company can read. Here it is
/// an ECIES-sealed envelope in the recipient's inbox, identical on the wire
/// to a message. Anyone replicating the inbox sees ciphertext of a
/// particular size and learns nothing about whether it was a tap, from whom,
/// or to whom.
///
/// That also means taps cost the same as messages and are rate-limited by
/// the same contract caps, which is the correct incentive: a tap that is
/// free to send at scale is a spam vector.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Tap,
}

/// First byte of a sealed payload, naming what follows.
///
/// A payload with no recognised marker is treated as plain UTF-8 text —
/// the format used before taps existed. Keeping that path costs one branch
/// and means an early message stays readable instead of turning into a
/// silent decode failure that looks exactly like a delivery bug.
const MARK_TEXT: u8 = 0x01;
const MARK_TAP: u8 = 0x02;

/// Encode a payload for sealing.
pub fn encode_payload(kind: Kind, body: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 1);
    out.push(match kind {
        Kind::Text => MARK_TEXT,
        Kind::Tap => MARK_TAP,
    });
    out.extend_from_slice(body.as_bytes());
    out
}

/// Decode a payload, tolerating the pre-marker format.
fn decode_payload(bytes: &[u8]) -> Option<(Kind, String)> {
    match bytes.first() {
        Some(&MARK_TEXT) => String::from_utf8(bytes[1..].to_vec())
            .ok()
            .map(|s| (Kind::Text, s)),
        Some(&MARK_TAP) => Some((Kind::Tap, String::new())),
        // Legacy: the whole payload is the text.
        _ => String::from_utf8(bytes.to_vec())
            .ok()
            .map(|s| (Kind::Text, s)),
    }
}

/// One decrypted message, ready to render.
#[derive(Clone, PartialEq)]
pub struct Message {
    /// Pseudonym of the sender's epoch key: the same value the grid tile
    /// carries, so a message can be matched to a face without either side
    /// revealing a durable identity.
    pub from: [u8; 32],
    pub kind: Kind,
    pub body: String,
    pub sent_ms: u64,
    /// Whether we sent it. Local-only messages are ours by construction.
    pub outgoing: bool,
}

/// A conversation: everything exchanged with one epoch pseudonym.
#[derive(Clone, PartialEq)]
pub struct Thread {
    pub peer: [u8; 32],
    pub headline: String,
    pub messages: Vec<Message>,
}

impl Thread {
    pub fn last(&self) -> Option<&Message> {
        self.messages.last()
    }
}

/// Decrypt everything in an inbox state that we can open, and group it.
///
/// Envelopes that fail to open are **skipped silently**, not surfaced as
/// errors. An inbox is world-writable, so anyone can drop in random bytes;
/// rendering "3 messages failed to decrypt" would turn that into a free
/// notification channel for whoever wants to bother you.
pub fn threads_from_inbox(
    id: &Identity,
    state_bytes: &[u8],
    tiles: &[Tile],
    blocked: &[[u8; 32]],
) -> Vec<Thread> {
    if state_bytes.is_empty() {
        return Vec::new();
    }
    let state: InboxState = match ciborium::de::from_reader(state_bytes) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut by_peer: std::collections::BTreeMap<[u8; 32], Vec<Message>> = Default::default();

    for env in state.envelopes.values() {
        // Sender pseudonym is BLAKE3 of their epoch verifying key — exactly
        // how the grid derives a tile's pseudonym, so the two line up.
        let from: [u8; 32] = *blake3::hash(&env.sender_epoch_vk).as_bytes();
        if blocked.contains(&from) {
            continue;
        }
        let Ok(plain) = id.open_message(env) else {
            continue;
        };
        let Some((kind, body)) = decode_payload(&plain) else {
            continue;
        };
        by_peer.entry(from).or_default().push(Message {
            from,
            kind,
            body,
            sent_ms: env.sent_ms,
            outgoing: false,
        });
    }

    let mut out: Vec<Thread> = by_peer
        .into_iter()
        .map(|(peer, mut messages)| {
            // Sender-claimed time, so this ordering is a courtesy, not a
            // guarantee. A sender who lies only misorders their own thread.
            messages.sort_by_key(|m| m.sent_ms);
            let headline = tiles
                .iter()
                .find(|t| t.pseudonym == peer)
                .map(|t| t.headline.clone())
                .unwrap_or_else(|| "Someone nearby".to_string());
            Thread {
                peer,
                headline,
                messages,
            }
        })
        .collect();

    // Newest conversation first, which is what every messaging app does and
    // what people expect without being told.
    out.sort_by_key(|t| std::cmp::Reverse(t.last().map(|m| m.sent_ms).unwrap_or(0)));
    out
}

/// Fold per-epoch thread lists into one.
///
/// Inboxes rotate with the epoch, so the same conversation can have messages
/// sitting in two contracts at once — merging them here is what makes an
/// epoch rollover invisible to the person reading their messages, rather
/// than something that appears to split a conversation in half.
pub fn merge_threads(lists: Vec<Thread>) -> Vec<Thread> {
    let mut by_peer: std::collections::BTreeMap<[u8; 32], Thread> = Default::default();
    for th in lists {
        match by_peer.get_mut(&th.peer) {
            Some(existing) => existing.messages.extend(th.messages),
            None => {
                by_peer.insert(th.peer, th);
            }
        }
    }
    let mut out: Vec<Thread> = by_peer.into_values().collect();
    for th in &mut out {
        th.messages.sort_by_key(|m| m.sent_ms);
        // The same envelope can legitimately reach us twice; showing a
        // message twice reads as a bug in a way a missing one does not.
        th.messages.dedup_by(|a, b| a.sent_ms == b.sent_ms && a.body == b.body);
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.last().map(|m| m.sent_ms).unwrap_or(0)));
    out
}

#[derive(Debug)]
pub enum SendError {
    /// Their tile carries no encryption key: an older client, or a record
    /// whose key failed validation.
    NotReachable,
    TooLong,
    Seal(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NotReachable => {
                write!(f, "this person's app can't receive messages yet")
            }
            SendError::TooLong => write!(f, "that message is too long"),
            SendError::Seal(e) => write!(f, "could not encrypt: {e}"),
        }
    }
}

/// Room for the ciphertext overhead inside the contract's per-envelope cap.
const MAX_BODY_BYTES: usize = 2 * 1024;

/// Seal a message to a tile, returning the envelope and the params of the
/// inbox contract it belongs in.
///
/// The recipient is addressed by the key **on their signed tile**. That
/// signature was verified before the tile was built, so a relay cannot have
/// swapped the key for its own — which is the entire reason the key is
/// inside the signed payload rather than beside it.
pub fn seal_to_tile(
    id: &Identity,
    tile: &Tile,
    epoch: u64,
    kind: Kind,
    body: &str,
    now_ms: u64,
) -> Result<(Envelope, InboxParams), SendError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(SendError::TooLong);
    }
    let enc = tile.encryption_key.ok_or(SendError::NotReachable)?;

    // Their inbox is addressed by the *epoch* key their tile is presenting,
    // because that is the only key we can possibly know. Addressing it by a
    // durable key fails on the network with "signature verification failed":
    // the envelope would be bound to one key and the contract to another.
    let recipient_vk = tile_verifying_key(tile).ok_or(SendError::NotReachable)?;

    let payload = encode_payload(kind, body);
    let env = id
        .seal_message(&enc, &recipient_vk, epoch, &payload, now_ms)
        .map_err(|e| SendError::Seal(e.to_string()))?;
    let params = InboxParams::new(&recipient_vk);
    Ok((env, params))
}

/// The verifying key behind a tile's pseudonym.
///
/// A tile stores the pseudonym (a hash), not the key, so this is only
/// available while the originating record is still in hand. Returning
/// `None` is correct and must stay a disabled Send button, never a
/// fallback to some weaker addressing scheme.
fn tile_verifying_key(tile: &Tile) -> Option<Vec<u8>> {
    tile.verifying_key.clone()
}
