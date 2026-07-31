//! Response demultiplexer.
//!
//! The node speaks one WebSocket for everything: replies to our requests
//! *and* unsolicited `UpdateNotification`s for every contract we're
//! subscribed to. Those interleave freely — a notification for cell A can
//! land between our GET request for cell B and its reply.
//!
//! [`FreenetClient`](crate::FreenetClient) copes by scanning responses
//! until it finds its own, which is fine for one-shot calls but discards
//! notifications and cannot support a live grid. This module inverts that:
//! a single background task owns `recv()` forever and routes each response
//! to whoever is waiting —
//!
//! * a **oneshot** for a caller awaiting a specific reply, matched by
//!   contract instance id and response kind;
//! * a **broadcast per contract** for subscribers, so N grid views can
//!   watch the same cell without contending for the socket.
//!
//! Everything below is `Send`-friendly and holds no lock across `.await`
//! on the socket, so a slow subscriber can never stall the reader.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use freenet_stdlib::client_api::{ClientRequest, ContractResponse, HostResponse, WebApi};
use freenet_stdlib::prelude::ContractInstanceId;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::FreenetError;

const NOTIFY_CAPACITY: usize = 64;

/// Which reply a waiter is interested in. Matching on kind as well as key
/// matters: a PUT and a GET for the same contract can be in flight at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReplyKind {
    Put,
    Get,
    Update,
    Subscribe,
}

/// What a waiter receives.
#[derive(Debug, Clone)]
pub enum Reply {
    Put,
    Get(Vec<u8>),
    Update,
    Subscribe,
}

/// A state change on a subscribed contract.
#[derive(Debug, Clone)]
pub enum Notification {
    /// New merged state (or the delta bytes, when that is all the node sends).
    Updated(Vec<u8>),
    /// The reader task ended — the session is gone and callers must reconnect.
    Closed,
}

type WaiterKey = (ContractInstanceId, ReplyKind);

#[derive(Default)]
struct Routes {
    waiters: HashMap<WaiterKey, Vec<oneshot::Sender<Reply>>>,
    subs: HashMap<ContractInstanceId, broadcast::Sender<Notification>>,
}

/// Handle to a running session with a demultiplexed reader.
#[derive(Clone)]
pub struct Demux {
    tx: mpsc::Sender<ClientRequest<'static>>,
    routes: Arc<Mutex<Routes>>,
}

impl Demux {
    /// Take ownership of `api` and spawn the reader.
    pub fn spawn(api: WebApi) -> Self {
        let routes: Arc<Mutex<Routes>> = Default::default();
        let (tx, rx) = mpsc::channel::<ClientRequest<'static>>(32);
        tokio::spawn(reader_loop(api, rx, routes.clone()));
        Self { tx, routes }
    }

    /// Register interest in a reply *before* sending the request, so a
    /// fast reply cannot arrive before we are listening for it.
    pub fn expect(&self, id: ContractInstanceId, kind: ReplyKind) -> oneshot::Receiver<Reply> {
        let (tx, rx) = oneshot::channel();
        self.routes
            .lock()
            .expect("routes mutex")
            .waiters
            .entry((id, kind))
            .or_default()
            .push(tx);
        rx
    }

    /// Send a request to the node.
    pub async fn send(&self, req: ClientRequest<'static>) -> Result<(), FreenetError> {
        self.tx
            .send(req)
            .await
            .map_err(|_| FreenetError::Api("session reader has stopped".into()))
    }

    /// Subscribe to notifications for one contract. Multiple callers may
    /// watch the same contract; each gets its own receiver.
    pub fn notifications(&self, id: ContractInstanceId) -> broadcast::Receiver<Notification> {
        self.routes
            .lock()
            .expect("routes mutex")
            .subs
            .entry(id)
            .or_insert_with(|| broadcast::channel(NOTIFY_CAPACITY).0)
            .subscribe()
    }
}

async fn reader_loop(
    mut api: WebApi,
    mut requests: mpsc::Receiver<ClientRequest<'static>>,
    routes: Arc<Mutex<Routes>>,
) {
    loop {
        tokio::select! {
            // Outgoing requests share this task so `api` has a single owner.
            Some(req) = requests.recv() => {
                if api.send(req).await.is_err() {
                    break;
                }
            }
            incoming = api.recv() => {
                match incoming {
                    Ok(resp) => route(&routes, resp),
                    Err(_) => break,
                }
            }
            else => break,
        }
    }
    // Tell every subscriber the session is gone; waiters simply see their
    // oneshot dropped, which surfaces as a receive error.
    let subs: Vec<_> = {
        let r = routes.lock().expect("routes mutex");
        r.subs.values().cloned().collect()
    };
    for s in subs {
        let _ = s.send(Notification::Closed);
    }
}

fn route(routes: &Arc<Mutex<Routes>>, resp: HostResponse) {
    let HostResponse::ContractResponse(cr) = resp else {
        return;
    };
    let (id, kind, reply) = match cr {
        ContractResponse::PutResponse { key } => (*key.id(), ReplyKind::Put, Reply::Put),
        ContractResponse::GetResponse { key, state, .. } => (
            *key.id(),
            ReplyKind::Get,
            Reply::Get(state.as_ref().to_vec()),
        ),
        ContractResponse::UpdateResponse { key, .. } => {
            (*key.id(), ReplyKind::Update, Reply::Update)
        }
        ContractResponse::SubscribeResponse { key, .. } => {
            (*key.id(), ReplyKind::Subscribe, Reply::Subscribe)
        }
        ContractResponse::UpdateNotification { key, update } => {
            let bytes = match update {
                freenet_stdlib::prelude::UpdateData::State(s) => s.as_ref().to_vec(),
                freenet_stdlib::prelude::UpdateData::Delta(d) => d.as_ref().to_vec(),
                freenet_stdlib::prelude::UpdateData::StateAndDelta { state, .. } => {
                    state.as_ref().to_vec()
                }
                _ => Vec::new(),
            };
            let sender = {
                let r = routes.lock().expect("routes mutex");
                r.subs.get(key.id()).cloned()
            };
            if let Some(s) = sender {
                // Err means nobody is listening right now, which is normal.
                let _ = s.send(Notification::Updated(bytes));
            }
            return;
        }
        _ => return,
    };

    // Wake the oldest waiter for this (contract, kind). FIFO keeps
    // concurrent identical requests from stealing each other's replies.
    let waiter = {
        let mut r = routes.lock().expect("routes mutex");
        r.waiters.get_mut(&(id, kind)).and_then(|v| {
            if v.is_empty() {
                None
            } else {
                Some(v.remove(0))
            }
        })
    };
    if let Some(w) = waiter {
        let _ = w.send(reply);
    }
}
