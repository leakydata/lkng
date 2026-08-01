//! Live grid: subscribe to a cell, then watch a tile arrive in real time.
//!
//! Usage: live_grid <wasm> <params.bin> <state.bin> <delta.bin>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{StateDelta, UpdateData, WrappedState, RelatedContracts};
use lkng_presence::CellState;
use lkng_transport_freenet::demux::{Demux, Notification, ReplyKind};
use lkng_transport_freenet::{FreenetClient, DEFAULT_NODE_URL};

fn tiles(bytes: &[u8]) -> usize {
    ciborium::de::from_reader::<CellState, _>(bytes)
        .map(|c| c.records.len())
        .unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (code, params, state, delta) = (
        std::fs::read(&a[0])?, std::fs::read(&a[1])?,
        std::fs::read(&a[2])?, std::fs::read(&a[3])?,
    );
    let key = FreenetClient::key_for(&code, &params);
    let id = *key.id();

    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));
    println!("session up, reader demultiplexing");

    // Seed (also subscribes), waiting on the demultiplexed reply.
    let put = demux.expect(id, ReplyKind::Put);
    demux.send(ClientRequest::ContractOp(ContractRequest::Put {
        contract: FreenetClient::container(&code, &params),
        state: WrappedState::new(state),
        related_contracts: RelatedContracts::default(),
        subscribe: true,
        blocking_subscribe: false,
    })).await?;
    tokio::time::timeout(Duration::from_secs(120), put).await??;
    println!("seeded + subscribed: {key}");

    // Start watching BEFORE writing, the way a grid view would.
    let mut watch = demux.notifications(id);
    let watcher = tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_secs(90), watch.recv()).await {
            Ok(Ok(Notification::Updated(b))) =>
                println!(">>> LIVE NOTIFICATION: {} bytes, {} tile(s) in the grid", b.len(), tiles(&b)),
            Ok(Ok(Notification::Closed)) => println!("session closed"),
            Ok(Err(e)) => println!("watch error: {e}"),
            Err(_) => println!("(no notification within 90s)"),
        }
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    let upd = demux.expect(id, ReplyKind::Update);
    demux.send(ClientRequest::ContractOp(ContractRequest::Update {
        key,
        data: UpdateData::Delta(StateDelta::from(delta)),
    })).await?;
    tokio::time::timeout(Duration::from_secs(120), upd).await??;
    println!("update accepted; waiting for the notification to land...");

    watcher.await?;
    demux.close().await;
    Ok(())
}
