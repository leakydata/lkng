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
