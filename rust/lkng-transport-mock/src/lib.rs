//! In-memory [`Transport`] for tests and UI development.
//!
//! Deterministic, no network, no crypto. Two deliberate simplifications,
//! both to be tightened when Phase 1 contract plumbing lands:
//!
//! * `publish` treats the delta as the *full new state* (real contracts
//!   merge; the mock replaces). Merge-semantics tests belong to the
//!   contract crates, not here.
//! * `sign`/`verify` are blake3 tags, NOT cryptography. They exist so call
//!   sites can be written and tested; the real backend delegates to the
//!   identity delegate.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures::stream::BoxStream;
use futures::StreamExt;
use lkng_transport::{
    ContentHash, Delta, PublicKey, Result, Signature, StateEvent, StateKey, StateSnapshot,
    Transport, TransportError,
};
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 64;

#[derive(Default)]
struct Inner {
    states: HashMap<StateKey, StateSnapshot>,
    blobs: HashMap<ContentHash, Bytes>,
    watchers: HashMap<StateKey, broadcast::Sender<StateEvent>>,
}

/// In-memory transport. Cheap to clone; clones share state.
#[derive(Clone, Default)]
pub struct MockTransport {
    inner: Arc<Mutex<Inner>>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl Transport for MockTransport {
    async fn publish(&self, key: &StateKey, delta: Delta) -> Result<()> {
        let snapshot = StateSnapshot(delta.0);
        let mut inner = self.inner.lock().expect("mock lock");
        inner.states.insert(key.clone(), snapshot.clone());
        if let Some(tx) = inner.watchers.get(key) {
            // Lagging or dropped receivers are fine — send only fails when
            // nobody is listening, which is not an error for a publisher.
            let _ = tx.send(StateEvent::Updated(snapshot));
        }
        Ok(())
    }

    async fn get(&self, key: &StateKey) -> Result<StateSnapshot> {
        self.inner
            .lock()
            .expect("mock lock")
            .states
            .get(key)
            .cloned()
            .ok_or(TransportError::NotFound)
    }

    async fn subscribe(&self, key: &StateKey) -> Result<BoxStream<'static, StateEvent>> {
        let rx = {
            let mut inner = self.inner.lock().expect("mock lock");
            inner
                .watchers
                .entry(key.clone())
                .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
                .subscribe()
        };
        let stream = futures::stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                Ok(event) => Some((event, rx)),
                // A lagged receiver missed updates: surface as Lapsed so the
                // caller re-reads, mirroring real resubscribe semantics.
                Err(broadcast::error::RecvError::Lagged(_)) => Some((StateEvent::Lapsed, rx)),
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });
        Ok(stream.boxed())
    }

    async fn put_blob(&self, bytes: Bytes) -> Result<ContentHash> {
        let hash = ContentHash(*blake3::hash(&bytes).as_bytes());
        self.inner
            .lock()
            .expect("mock lock")
            .blobs
            .insert(hash, bytes);
        Ok(hash)
    }

    async fn get_blob(&self, hash: &ContentHash) -> Result<Bytes> {
        self.inner
            .lock()
            .expect("mock lock")
            .blobs
            .get(hash)
            .cloned()
            .ok_or(TransportError::NotFound)
    }

    async fn sign(&self, payload: &[u8]) -> Result<Signature> {
        Ok(Signature(blake3::hash(payload).as_bytes().to_vec()))
    }

    async fn verify(&self, payload: &[u8], signature: &Signature, _key: &PublicKey) -> Result<bool> {
        Ok(signature.0 == blake3::hash(payload).as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> StateKey {
        StateKey(name.as_bytes().to_vec())
    }

    #[tokio::test]
    async fn publish_then_get_roundtrips() {
        let t = MockTransport::new();
        let k = key("profile:alice");
        t.publish(&k, Delta(Bytes::from_static(b"v1"))).await.unwrap();
        assert_eq!(t.get(&k).await.unwrap().0, Bytes::from_static(b"v1"));
    }

    #[tokio::test]
    async fn get_missing_is_not_found() {
        let t = MockTransport::new();
        assert!(matches!(
            t.get(&key("nope")).await,
            Err(TransportError::NotFound)
        ));
    }

    #[tokio::test]
    async fn subscriber_sees_update() {
        let t = MockTransport::new();
        let k = key("presence:cell:epoch");
        let mut stream = t.subscribe(&k).await.unwrap();
        t.publish(&k, Delta(Bytes::from_static(b"rec"))).await.unwrap();
        match stream.next().await {
            Some(StateEvent::Updated(s)) => assert_eq!(s.0, Bytes::from_static(b"rec")),
            other => panic!("expected Updated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn blobs_are_content_addressed() {
        let t = MockTransport::new();
        let h1 = t.put_blob(Bytes::from_static(b"thumb")).await.unwrap();
        let h2 = t.put_blob(Bytes::from_static(b"thumb")).await.unwrap();
        assert_eq!(h1, h2, "same content, same hash");
        assert_eq!(t.get_blob(&h1).await.unwrap(), Bytes::from_static(b"thumb"));
    }

    #[tokio::test]
    async fn mock_sign_verifies() {
        let t = MockTransport::new();
        let sig = t.sign(b"hello").await.unwrap();
        assert!(t
            .verify(b"hello", &sig, &PublicKey(vec![]))
            .await
            .unwrap());
        assert!(!t
            .verify(b"tampered", &sig, &PublicKey(vec![]))
            .await
            .unwrap());
    }
}
