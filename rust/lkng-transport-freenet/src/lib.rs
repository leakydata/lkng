//! Freenet backend for [`lkng_transport::Transport`].
//!
//! Two things distinguish this from naive use of the client API, both
//! learned the hard way (see `docs/gate-status.md`):
//!
//! ## One persistent session
//!
//! Every `fdev execute …` invocation opens a fresh WebSocket. That is why
//! `publish` in one process and `update` in the next fails with
//! `missing contract`: the node associates hosted contracts with the
//! client session that seeded them. River keeps a single long-lived
//! `WebApi` for the app's lifetime and issues every request through it.
//! So does this — [`FreenetClient`] owns one connection, and the same
//! connection is used for PUT, GET, SUBSCRIBE and UPDATE.
//!
//! ## Seed-PUT before update
//!
//! An UPDATE is applied *locally before forwarding*, because the wire
//! format carries a post-merge full state rather than a raw delta. The
//! originator therefore needs the contract's code and parameters in its
//! own store. River handles this by PUTting the full
//! `ContractContainer` (code + params + state) with `subscribe: true`
//! before it ever updates — its own comments call this the "seed PUT".
//! [`FreenetClient::seed`] does the same.

pub mod demux;

use std::collections::HashMap;
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, HostResponse, WebApi};
use freenet_stdlib::prelude::{
    ContractCode, ContractContainer, ContractInstanceId, ContractKey, ContractWasmAPIVersion,
    Parameters, RelatedContracts, StateDelta, UpdateData, WrappedContract, WrappedState,
};
use std::sync::Arc;

pub const DEFAULT_NODE_URL: &str = "ws://127.0.0.1:7509/v1/contract/command?encodingProtocol=native";

#[derive(Debug, thiserror::Error)]
pub enum FreenetError {
    #[error("websocket: {0}")]
    Connect(String),
    #[error("client api: {0}")]
    Api(String),
    #[error("node reported: {0}")]
    Node(String),
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("contract not found")]
    NotFound,
}

type Result<T> = std::result::Result<T, FreenetError>;

/// A single long-lived client session against one node.
pub struct FreenetClient {
    api: WebApi,
    timeout: Duration,
}

impl FreenetClient {
    /// Connect to a node's client API. `url` is typically
    /// [`DEFAULT_NODE_URL`]; on Android it is the loopback port the
    /// bundled node was started on, plus its session token.
    pub async fn connect(url: &str, timeout: Duration) -> Result<Self> {
        let (stream, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| FreenetError::Connect(e.to_string()))?;
        Ok(Self {
            api: WebApi::start(stream),
            timeout,
        })
    }

    async fn request(&mut self, req: ClientRequest<'static>) -> Result<()> {
        self.api
            .send(req)
            .await
            .map_err(|e| FreenetError::Api(e.to_string()))
    }

    /// Await responses until `pick` returns something, or we time out.
    ///
    /// Responses are not strictly request-ordered (update notifications
    /// can arrive at any moment on a subscribed contract), so callers
    /// match on the response they want rather than assuming the next one
    /// is theirs.
    async fn await_response<T>(
        &mut self,
        what: &'static str,
        mut pick: impl FnMut(&HostResponse) -> Option<T>,
    ) -> Result<T> {
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(FreenetError::Timeout(what));
            }
            let recv = tokio::time::timeout(remaining, self.api.recv()).await;
            match recv {
                Err(_) => return Err(FreenetError::Timeout(what)),
                Ok(Err(e)) => return Err(FreenetError::Node(e.to_string())),
                Ok(Ok(resp)) => {
                    if let Some(found) = pick(&resp) {
                        return Ok(found);
                    }
                }
            }
        }
    }

    /// Build a contract container from code + parameters.
    pub fn container(code: &[u8], params: &[u8]) -> ContractContainer {
        ContractContainer::from(ContractWasmAPIVersion::V1(WrappedContract::new(
            Arc::new(ContractCode::from(code.to_vec())),
            Parameters::from(params.to_vec()),
        )))
    }

    /// Contract key for code + parameters, without touching the network.
    pub fn key_for(code: &[u8], params: &[u8]) -> ContractKey {
        match Self::container(code, params) {
            ContractContainer::Wasm(ContractWasmAPIVersion::V1(w)) => *w.key(),
            _ => unreachable!("we just built a V1 wasm container"),
        }
    }

    /// **Seed PUT** — publish code + parameters + state and subscribe.
    ///
    /// This is the step that makes a later [`FreenetClient::update`]
    /// possible: it puts the contract into *this node's* store so the
    /// local pre-forward merge can run. Safe to call when the contract
    /// already exists; the node treats it as an ordinary PUT.
    pub async fn seed(
        &mut self,
        code: &[u8],
        params: &[u8],
        state: Vec<u8>,
    ) -> Result<ContractKey> {
        let container = Self::container(code, params);
        let key = match &container {
            ContractContainer::Wasm(ContractWasmAPIVersion::V1(w)) => *w.key(),
            _ => unreachable!(),
        };
        self.request(ClientRequest::ContractOp(ContractRequest::Put {
            contract: container,
            state: WrappedState::new(state),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
        let target = *key.id();
        self.await_response("put", |r| match r {
            HostResponse::ContractResponse(
                freenet_stdlib::client_api::ContractResponse::PutResponse { key },
            ) if *key.id() == target => Some(()),
            _ => None,
        })
        .await?;
        Ok(key)
    }

    /// Fetch current state.
    pub async fn get(&mut self, key: &ContractKey, subscribe: bool) -> Result<Vec<u8>> {
        self.request(ClientRequest::ContractOp(ContractRequest::Get {
            key: (*key).into(),
            return_contract_code: false,
            subscribe,
            blocking_subscribe: false,
        }))
        .await?;
        let target = *key.id();
        self.await_response("get", |r| match r {
            HostResponse::ContractResponse(
                freenet_stdlib::client_api::ContractResponse::GetResponse { key, state, .. },
            ) if *key.id() == target => Some(state.as_ref().to_vec()),
            _ => None,
        })
        .await
    }

    /// Apply a delta to a contract. Requires a prior [`FreenetClient::seed`]
    /// **on this same session** for contracts this node does not host.
    pub async fn update(&mut self, key: &ContractKey, delta: Vec<u8>) -> Result<()> {
        self.request(ClientRequest::ContractOp(ContractRequest::Update {
            key: (*key).into(),
            data: UpdateData::Delta(StateDelta::from(delta)),
        }))
        .await?;
        let target: ContractInstanceId = *key.id();
        self.await_response("update", |r| match r {
            HostResponse::ContractResponse(
                freenet_stdlib::client_api::ContractResponse::UpdateResponse { key, .. },
            ) if *key.id() == target => Some(()),
            _ => None,
        })
        .await
    }

    /// Subscribe without fetching.
    pub async fn subscribe(&mut self, key: &ContractKey) -> Result<()> {
        self.request(ClientRequest::ContractOp(ContractRequest::Subscribe {
            key: (*key).into(),
            summary: None,
        }))
        .await?;
        let target = *key.id();
        self.await_response("subscribe", |r| match r {
            HostResponse::ContractResponse(
                freenet_stdlib::client_api::ContractResponse::SubscribeResponse { key, .. },
            ) if *key.id() == target => Some(()),
            _ => None,
        })
        .await
    }
}

/// Everything needed to seed a contract before writing to it: its compiled
/// code and its parameters. Registered up-front by the app so the
/// [`Transport`] impl can seed on demand without app code knowing that
/// seeding exists.
#[derive(Clone)]
pub struct ContractSpec {
    pub code: Vec<u8>,
    pub params: Vec<u8>,
}

/// [`lkng_transport::Transport`] over a live Freenet node.
///
/// The trait is deliberately backend-agnostic — it knows nothing about
/// seed PUTs, sessions or contract code. This adapter absorbs all of it:
///
/// * one [`FreenetClient`] session for the object's lifetime,
/// * a registry of [`ContractSpec`]s so a `publish` to a key we have code
///   for can seed first, automatically, exactly once per key.
///
/// The blob methods are intentionally unimplemented for now: media lands
/// in Phase 4 and will use freenet-git's ChunkedPack rather than anything
/// invented here. Failing loudly beats a plausible stub.
pub struct FreenetTransport {
    inner: tokio::sync::Mutex<Session>,
}

struct Session {
    client: FreenetClient,
    /// StateKey bytes -> everything we know about that contract. A
    /// `ContractKey` carries both an instance id and a code hash, and the
    /// instance id alone cannot reconstruct it — so the registry holds the
    /// derived key rather than rebuilding it per call.
    known: HashMap<Vec<u8>, Known>,
    seeded: std::collections::HashSet<Vec<u8>>,
}

#[derive(Clone)]
struct Known {
    spec: ContractSpec,
    key: ContractKey,
}

impl FreenetTransport {
    pub async fn connect(url: &str, timeout: Duration) -> Result<Self> {
        let client = FreenetClient::connect(url, timeout).await?;
        Ok(Self {
            inner: tokio::sync::Mutex::new(Session {
                client,
                known: HashMap::new(),
                seeded: Default::default(),
            }),
        })
    }

    /// Register a contract's code and parameters, returning the
    /// [`lkng_transport::StateKey`] app code should use from then on.
    ///
    /// Registration is mandatory, not an optimisation: an UPDATE requires
    /// a seed PUT carrying the code, and a `ContractKey` cannot be rebuilt
    /// from an instance id alone. Anything unregistered is rejected with a
    /// message that says so, rather than failing later as
    /// `missing contract`.
    pub async fn register_contract(
        &self,
        code: Vec<u8>,
        params: Vec<u8>,
    ) -> lkng_transport::StateKey {
        let key = FreenetClient::key_for(&code, &params);
        let sk = lkng_transport::StateKey(key.id().as_bytes().to_vec());
        self.inner.lock().await.known.insert(
            sk.0.clone(),
            Known { spec: ContractSpec { code, params }, key },
        );
        sk
    }
}

impl Session {
    fn lookup(&self, sk: &lkng_transport::StateKey) -> std::result::Result<Known, FreenetError> {
        self.known.get(&sk.0).cloned().ok_or_else(|| {
            FreenetError::Api(
                "contract not registered — call register_contract(code, params) first".into(),
            )
        })
    }
}

#[async_trait::async_trait]
impl lkng_transport::Transport for FreenetTransport {
    async fn publish(
        &self,
        key: &lkng_transport::StateKey,
        delta: lkng_transport::Delta,
    ) -> lkng_transport::Result<()> {
        let mut s = self.inner.lock().await;
        let known = s.lookup(key).map_err(to_transport_err)?;

        // First write to a key seeds it; later writes are deltas. This is
        // the whole reason the registry exists — app code just calls
        // publish() and never learns that seeding is a thing.
        if !s.seeded.contains(&key.0) {
            s.client
                .seed(&known.spec.code, &known.spec.params, delta.0.to_vec())
                .await
                .map_err(to_transport_err)?;
            s.seeded.insert(key.0.clone());
            // The seed PUT already carried this state.
            return Ok(());
        }
        s.client
            .update(&known.key, delta.0.to_vec())
            .await
            .map_err(to_transport_err)
    }

    async fn get(
        &self,
        key: &lkng_transport::StateKey,
    ) -> lkng_transport::Result<lkng_transport::StateSnapshot> {
        let mut s = self.inner.lock().await;
        let known = s.lookup(key).map_err(to_transport_err)?;
        let bytes = s
            .client
            .get(&known.key, false)
            .await
            .map_err(to_transport_err)?;
        Ok(lkng_transport::StateSnapshot(bytes.into()))
    }

    async fn subscribe(
        &self,
        key: &lkng_transport::StateKey,
    ) -> lkng_transport::Result<futures::stream::BoxStream<'static, lkng_transport::StateEvent>> {
        let mut s = self.inner.lock().await;
        let known = s.lookup(key).map_err(to_transport_err)?;
        s.client
            .subscribe(&known.key)
            .await
            .map_err(to_transport_err)?;
        // Update notifications arrive on the shared session; routing them
        // to per-key streams needs a demultiplexer that owns `recv`, which
        // lands with the UI work. Until then a subscription is registered
        // with the node (which is what keeps the contract hot) and callers
        // poll via `get`.
        Ok(Box::pin(futures::stream::empty()))
    }

    async fn put_blob(
        &self,
        _bytes: bytes::Bytes,
    ) -> lkng_transport::Result<lkng_transport::ContentHash> {
        Err(lkng_transport::TransportError::Unavailable(
            "blob storage lands in Phase 4 (freenet-git ChunkedPack)".into(),
        ))
    }

    async fn get_blob(
        &self,
        _hash: &lkng_transport::ContentHash,
    ) -> lkng_transport::Result<bytes::Bytes> {
        Err(lkng_transport::TransportError::Unavailable(
            "blob storage lands in Phase 4 (freenet-git ChunkedPack)".into(),
        ))
    }

    async fn sign(&self, _payload: &[u8]) -> lkng_transport::Result<lkng_transport::Signature> {
        // Signing belongs to the identity delegate, which holds the key.
        // Routing it through the transport would put key access on the
        // network path — exactly the boundary the delegate exists to keep.
        Err(lkng_transport::TransportError::Crypto(
            "sign via lkng-identity / the identity delegate, not the transport".into(),
        ))
    }

    async fn verify(
        &self,
        _payload: &[u8],
        _signature: &lkng_transport::Signature,
        _key: &lkng_transport::PublicKey,
    ) -> lkng_transport::Result<bool> {
        Err(lkng_transport::TransportError::Crypto(
            "verify via lkng-presence::verify / lkng-profile::verify".into(),
        ))
    }
}

fn to_transport_err(e: FreenetError) -> lkng_transport::TransportError {
    use lkng_transport::TransportError as T;
    match e {
        FreenetError::NotFound => T::NotFound,
        FreenetError::Timeout(w) => T::Unavailable(format!("timed out waiting for {w}")),
        FreenetError::Connect(m) => T::Unavailable(m),
        FreenetError::Node(m) if m.contains("missing contract") => T::Rejected(format!(
            "{m} — the contract was not seeded on this session; register() its code and params"
        )),
        FreenetError::Node(m) => T::Rejected(m),
        FreenetError::Api(m) => T::Unavailable(m),
    }
}
