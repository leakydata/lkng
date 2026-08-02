//! Browser-side connection to the local Freenet node.
//!
//! The browser `WebApi` is callback-driven rather than await-driven (there
//! is no socket to own on a single thread), so this module inverts it into
//! something the UI can poll: responses land in a shared buffer and the
//! grid reads whatever has arrived.
//!
//! The write path follows the same rule proved on the desktop side: **seed
//! PUT with code and parameters, on the session that will later update**.
//! A browser session is naturally long-lived — it lasts as long as the tab
//! — which is exactly the shape that makes multi-writer contracts work.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use freenet_stdlib::client_api::{
    ClientError, ClientRequest, ContractRequest, ContractResponse, HostResponse, WebApi,
};
use freenet_stdlib::prelude::{
    ContractCode, ContractContainer, ContractInstanceId, ContractKey, ContractWasmAPIVersion,
    Parameters, RelatedContracts, StateDelta, UpdateData, WrappedContract, WrappedState,
};

/// Where the node's client API lives.
///
/// Derived from the page location when the app is served by the node
/// itself (the production path, and how Delta does it), falling back to
/// the default loopback port during `dx serve` development.
pub fn node_url() -> String {
    let win = web_sys::window();
    let loc = win.as_ref().map(|w| w.location());
    let host = loc
        .as_ref()
        .and_then(|l| l.host().ok())
        .unwrap_or_else(|| "127.0.0.1:7509".to_string());
    let proto = loc
        .as_ref()
        .and_then(|l| l.protocol().ok())
        .unwrap_or_else(|| "http:".into());
    let ws = if proto == "https:" { "wss" } else { "ws" };
    // Served by the node → same host, which is the production path and
    // works inside the node's sandbox iframe. Served by `dx serve` → that
    // dev port has no node behind it, so fall back to the node's default.
    let host = if host.contains(":8080") {
        "127.0.0.1:7509".to_string()
    } else {
        host
    };
    format!("{ws}://{host}/v1/contract/command?encodingProtocol=native")
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Connecting,
    Connected,
    Failed(String),
}

impl Status {
    /// Short human explanation, used in the UI. A status line that only
    /// ever says "connecting" is indistinguishable from a hang.
    pub fn describe(&self, url: &str) -> String {
        match self {
            Status::Connected => "Live from the network.".into(),
            Status::Connecting => format!("Connecting to your node at {url}…"),
            Status::Failed(e) => format!("No node at {url}: {e}"),
        }
    }
}

/// What the UI polls.
#[derive(Default)]
pub struct Inbox {
    /// Latest state seen per contract, whether from a GET reply or a
    /// pushed update notification — the UI does not care which.
    pub states: HashMap<ContractInstanceId, Vec<u8>>,
    /// Contracts confirmed present after a PUT, so we only seed once.
    pub seeded: Vec<ContractInstanceId>,
    pub last_error: Option<String>,
    pub generation: u64,
}

#[derive(Clone)]
pub struct Node {
    api: Rc<RefCell<Option<WebApi>>>,
    pub inbox: Rc<RefCell<Inbox>>,
    pub status: Rc<RefCell<Status>>,
}

impl Node {
    /// Open the connection. Returns immediately; watch [`Node::status`].
    pub fn connect() -> Self {
        let inbox: Rc<RefCell<Inbox>> = Rc::new(RefCell::new(Inbox::default()));
        let status = Rc::new(RefCell::new(Status::Connecting));
        let api: Rc<RefCell<Option<WebApi>>> = Rc::new(RefCell::new(None));

        let url = node_url();
        let socket = match web_sys::WebSocket::new(&url) {
            Ok(s) => s,
            Err(e) => {
                *status.borrow_mut() = Status::Failed(format!("{e:?}"));
                return Self { api, inbox, status };
            }
        };

        let (r_inbox, e_inbox) = (inbox.clone(), inbox.clone());
        let (o_status, e_status) = (status.clone(), status.clone());

        let web_api = WebApi::start(
            socket,
            move |result: Result<HostResponse, ClientError>| {
                let mut i = r_inbox.borrow_mut();
                match result {
                    Ok(HostResponse::ContractResponse(cr)) => absorb(&mut i, cr),
                    Ok(_) => {}
                    Err(e) => i.last_error = Some(e.to_string()),
                }
                i.generation += 1;
            },
            move |e| {
                e_inbox.borrow_mut().last_error = Some(e.to_string());
                *e_status.borrow_mut() = Status::Failed(e.to_string());
            },
            move || {
                *o_status.borrow_mut() = Status::Connected;
            },
        );
        *api.borrow_mut() = Some(web_api);
        Self { api, inbox, status }
    }

    fn request(&self, req: ClientRequest<'static>) {
        let api = self.api.clone();
        let inbox = self.inbox.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut guard = api.borrow_mut();
            if let Some(a) = guard.as_mut() {
                if let Err(e) = a.send(req).await {
                    inbox.borrow_mut().last_error = Some(e.to_string());
                }
            }
        });
    }

    pub fn key_for(code: &[u8], params: &[u8]) -> ContractKey {
        match ContractContainer::from(ContractWasmAPIVersion::V1(WrappedContract::new(
            std::sync::Arc::new(ContractCode::from(code.to_vec())),
            Parameters::from(params.to_vec()),
        ))) {
            ContractContainer::Wasm(ContractWasmAPIVersion::V1(w)) => *w.key(),
            _ => unreachable!(),
        }
    }

    /// Seed PUT — the step that makes later updates possible for a
    /// contract this node does not host.
    /// Seed a contract, but only the first time.
    ///
    /// Seeding exists to make a contract *exist* so a following update has
    /// somewhere to land. Once we have seen state for it, repeating the PUT
    /// buys nothing and costs everyone: presence is republished every few
    /// minutes, so an unconditional seed is a full contract container
    /// pushed into the network on a timer, forever, for a contract that is
    /// already there.
    ///
    /// Returns whether the seed was actually sent, so callers can tell
    /// "already present" from "just created" without asking again.
    pub fn seed_once(&self, code: &[u8], params: &[u8], state: Vec<u8>) -> bool {
        let key = Self::key_for(code, params);
        let id = *key.id();
        {
            let inbox = self.inbox.borrow();
            if inbox.seeded.contains(&id) || inbox.states.contains_key(&id) {
                return false;
            }
        }
        self.inbox.borrow_mut().seeded.push(id);
        self.seed(code, params, state);
        true
    }

    pub fn seed(&self, code: &[u8], params: &[u8], state: Vec<u8>) {
        let container = ContractContainer::from(ContractWasmAPIVersion::V1(WrappedContract::new(
            std::sync::Arc::new(ContractCode::from(code.to_vec())),
            Parameters::from(params.to_vec()),
        )));
        self.request(ClientRequest::ContractOp(ContractRequest::Put {
            contract: container,
            state: WrappedState::new(state),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }));
    }

    pub fn get(&self, key: ContractKey, subscribe: bool) {
        self.request(ClientRequest::ContractOp(ContractRequest::Get {
            key: key.into(),
            return_contract_code: false,
            subscribe,
            blocking_subscribe: false,
        }));
    }

    pub fn update(&self, key: ContractKey, delta: Vec<u8>) {
        self.request(ClientRequest::ContractOp(ContractRequest::Update {
            key: key.into(),
            data: UpdateData::Delta(StateDelta::from(delta)),
        }));
    }

    /// State currently known for a contract, if any.
    pub fn state_of(&self, id: &ContractInstanceId) -> Option<Vec<u8>> {
        self.inbox.borrow().states.get(id).cloned()
    }

    /// Bumped on every response, so the UI can cheaply detect "something
    /// arrived" without diffing the whole map.
    pub fn generation(&self) -> u64 {
        self.inbox.borrow().generation
    }
}

fn absorb(inbox: &mut Inbox, cr: ContractResponse) {
    match cr {
        ContractResponse::GetResponse { key, state, .. } => {
            inbox.states.insert(*key.id(), state.as_ref().to_vec());
        }
        ContractResponse::PutResponse { key } => {
            if !inbox.seeded.contains(key.id()) {
                inbox.seeded.push(*key.id());
            }
        }
        // A push. Full state replaces; a bare delta cannot be merged here
        // (that is the contract's job), so it triggers a re-GET instead of
        // being guessed at.
        ContractResponse::UpdateNotification { key, update } => match update {
            UpdateData::State(s) => {
                inbox.states.insert(*key.id(), s.as_ref().to_vec());
            }
            UpdateData::StateAndDelta { state, .. } => {
                inbox.states.insert(*key.id(), state.as_ref().to_vec());
            }
            _ => {}
        },
        _ => {}
    }
}
