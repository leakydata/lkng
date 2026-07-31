//! Backend-agnostic transport for LKNG.
//!
//! Everything above this trait (contract logic, delegates, UI glue) talks
//! *only* to [`Transport`]. Two implementations exist:
//!
//! * `lkng-transport-freenet` — the real backend, speaking the Freenet node's
//!   WebSocket client API (identical whether the endpoint is the local
//!   embedded node or a public gateway during warm start).
//! * `lkng-transport-mock` — in-memory, deterministic, no network. All UI
//!   work and contract-logic tests run against it, so nothing upstream ever
//!   blocks on a pre-1.0 P2P network.
//!
//! Keep this surface *narrow*: every method added here is a method the mock
//! must fake and any future backend must satisfy. If a feature can be built
//! from the existing methods, build it above the trait, not in it.

use bytes::Bytes;
use futures::stream::BoxStream;

/// Address of a piece of replicated state (a contract instance).
///
/// For the Freenet backend this wraps the contract key
/// (`hash(code, params)`); the mock treats it as an opaque map key. LKNG
/// code must never construct one from anything location-derived — see the
/// location assertion test in `tests/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StateKey(pub Vec<u8>);

/// Content hash addressing an immutable blob (media chunk, manifest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(pub [u8; 32]);

/// A serialized delta to merge into replicated state.
///
/// Contract semantics (commutative monoid — order-independent merges) are
/// the *contract's* job; the transport just moves bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta(pub Bytes);

/// A full serialized contract state as currently known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSnapshot(pub Bytes);

/// Verifying key bytes (ML-DSA-65 encoded verifying key in production).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey(pub Vec<u8>);

/// Detached signature bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature(pub Vec<u8>);

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The state does not exist (or, on Freenet, may have been evicted —
    /// callers must treat "not found" as recoverable and re-publish from
    /// local authoritative storage; see PLAN.md "Eviction").
    #[error("state not found for key")]
    NotFound,
    /// The backend rejected the update (contract validation failed).
    #[error("update rejected: {0}")]
    Rejected(String),
    /// Connection-level failure. Retryable.
    #[error("transport unavailable: {0}")]
    Unavailable(String),
    /// Signing/verification failure inside the identity layer.
    #[error("crypto error: {0}")]
    Crypto(String),
}

pub type Result<T> = std::result::Result<T, TransportError>;

/// Events delivered on a subscription.
#[derive(Debug, Clone)]
pub enum StateEvent {
    /// A delta was applied; the new merged snapshot is attached.
    Updated(StateSnapshot),
    /// The subscription lapsed (node restart, eviction, endpoint handoff).
    /// The caller should re-subscribe and reconcile.
    Lapsed,
}

/// The one interface LKNG application code may use to reach the network.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// Merge a delta into the state at `key`, creating it if absent.
    async fn publish(&self, key: &StateKey, delta: Delta) -> Result<()>;

    /// Fetch the current merged state at `key`.
    async fn get(&self, key: &StateKey) -> Result<StateSnapshot>;

    /// Subscribe to changes at `key`. The stream yields the merged state
    /// after each update. Dropping the stream unsubscribes.
    async fn subscribe(&self, key: &StateKey) -> Result<BoxStream<'static, StateEvent>>;

    /// Store an immutable blob, returning its content hash.
    async fn put_blob(&self, bytes: Bytes) -> Result<ContentHash>;

    /// Fetch an immutable blob by content hash.
    async fn get_blob(&self, hash: &ContentHash) -> Result<Bytes>;

    /// Sign `payload` with the local identity (delegated to the identity
    /// delegate in production — the key never crosses this boundary).
    async fn sign(&self, payload: &[u8]) -> Result<Signature>;

    /// Verify `signature` over `payload` against `key`.
    async fn verify(&self, payload: &[u8], signature: &Signature, key: &PublicKey) -> Result<bool>;
}
