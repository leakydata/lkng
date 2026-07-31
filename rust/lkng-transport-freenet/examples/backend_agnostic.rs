//! The point of the Transport trait: one piece of app logic, two backends.
//!
//! `publish_tile` below never mentions Freenet, seeding, sessions or
//! contract code. It runs unchanged against the in-memory mock and against
//! the live network.
use std::time::Duration;

use lkng_identity::Identity;
use lkng_presence::{CellParams, CellState, PresenceRecord};
use lkng_transport::{Delta, StateKey, Transport};
use lkng_transport_freenet::FreenetTransport;
use lkng_transport_mock::MockTransport;

/// Sign a tile and publish it. Backend-agnostic on purpose.
async fn publish_tile<T: Transport>(
    t: &T,
    key: &StateKey,
    id: &Identity,
    params: &CellParams,
    headline: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut rec = PresenceRecord {
        pseudonym: [0; 32],
        headline: headline.into(),
        thumbnail: vec![5u8; 64],
        timestamp_ms: 1_785_526_000_000,
        verifying_key: None,
        writer_cert: None,
        sig: vec![],
    };
    id.sign_presence(&mut rec, params)?;
    lkng_presence::verify::verify_self_contained(&rec, params)?;

    let mut cell = CellState::default();
    cell.insert(rec);
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&cell, &mut buf)?;

    t.publish(key, Delta(buf.into())).await?;
    let back = t.get(key).await?;
    Ok(back.0.len())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (code, params_bytes) = (std::fs::read(&a[0])?, std::fs::read(&a[1])?);
    let params: CellParams = ciborium::de::from_reader(&params_bytes[..])?;
    let id = Identity::from_seed([99u8; 32]);

    // --- backend A: in-memory mock, no network at all
    let mock = MockTransport::new();
    let mock_key = StateKey(b"presence:demo".to_vec());
    let n = publish_tile(&mock, &mock_key, &id, &params, "via mock").await?;
    println!("mock backend:    published + read back {n} bytes");

    // --- backend B: the live Freenet network, same function
    let fnet = FreenetTransport::connect(
        lkng_transport_freenet::DEFAULT_NODE_URL,
        Duration::from_secs(120),
    )
    .await?;
    let key = fnet.register_contract(code, params_bytes).await;
    let n = publish_tile(&fnet, &key, &id, &params, "via live network").await?;
    println!("freenet backend: published + read back {n} bytes");

    println!("\nsame publish_tile() ran on both — app code never saw a session or a seed PUT");
    Ok(())
}
