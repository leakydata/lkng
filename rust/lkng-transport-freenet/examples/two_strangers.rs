//! The whole product, end to end, on the live network.
//!
//! Two people who have never met:
//!
//! 1. both compute a publishable cell from their real GPS (jittered,
//!    on-device — raw coordinates never leave `lkng-location`),
//!    and discover they land in the same one;
//! 2. both post a tile signed by a **per-epoch** key;
//! 3. each sees the other's tile arrive, live;
//! 4. neither tile reveals a durable identity — verified by scanning the
//!    actual bytes on the wire;
//! 5. one of them then *chooses* to reveal their profile, and only then
//!    can the other find and verify it.
//!
//! Usage: two_strangers <cell.wasm> <cell_params.bin> <profile.wasm>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{RelatedContracts, StateDelta, UpdateData, WrappedState};
use lkng_identity::Identity;
use lkng_location::{publishable_cell, JitterRadius};
use lkng_presence::{verify::verify_self_contained, CellParams, CellState, PresenceRecord};
use lkng_profile::ProfileBody;
use lkng_transport_freenet::demux::{Demux, Notification, ReplyKind};
use lkng_transport_freenet::{FreenetClient, DEFAULT_NODE_URL};

fn cbor(v: &impl serde::Serialize) -> Vec<u8> {
    let mut b = Vec::new();
    ciborium::ser::into_writer(v, &mut b).expect("cbor");
    b
}

fn tile(id: &Identity, params: &CellParams, headline: &str, ts: u64) -> PresenceRecord {
    let mut r = PresenceRecord {
        pseudonym: [0; 32],
        headline: headline.into(),
        thumbnail: vec![0u8; 48],
        timestamp_ms: ts,
        verifying_key: None,
        writer_cert: None,
        sig: vec![],
    };
    id.sign_presence(&mut r, params).expect("sign tile");
    verify_self_contained(&r, params).expect("tile must verify before publishing");
    r
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cell_code = std::fs::read(&a[0])?;
    let cell_params_bytes = std::fs::read(&a[1])?;
    let profile_code = std::fs::read(&a[2])?;
    let params: CellParams = ciborium::de::from_reader(&cell_params_bytes[..])?;

    // Two strangers, two independent identities.
    let alex = Identity::from_seed([0xA1; 32]);
    let sam = Identity::from_seed([0x5A; 32]);

    // --- 1. location, jittered on-device -------------------------------
    // Both are near the same San Francisco block. Neither's true position
    // leaves this process: publishable_cell returns only a coarse cell.
    let secret_a = [0xAA; 32];
    let secret_s = [0x55; 32];
    let cell_a = publishable_cell(&secret_a, 37.7749, -122.4194, JitterRadius::Km1)?;
    let cell_s = publishable_cell(&secret_s, 37.7761, -122.4180, JitterRadius::Km1)?;
    println!("alex publishes cell {}", cell_a.as_str());
    println!("sam  publishes cell {}", cell_s.as_str());
    println!(
        "same cell: {}  (they can discover each other)",
        cell_a == cell_s
    );

    // --- 2. session + seed --------------------------------------------
    let key = FreenetClient::key_for(&cell_code, &cell_params_bytes);
    let id = *key.id();
    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));

    let mut opening = CellState::default();
    opening.insert(tile(&alex, &params, "alex: new here", 1_785_527_000_000));

    let put = demux.expect(id, ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&cell_code, &cell_params_bytes),
            state: WrappedState::new(cbor(&opening)),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    tokio::time::timeout(Duration::from_secs(120), put).await??;
    println!("\ncell live: {key}\nalex posted the first tile");

    // --- 3. sam arrives; alex is watching ------------------------------
    let mut watch = demux.notifications(id);
    let watcher = tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_secs(90), watch.recv()).await {
            Ok(Ok(Notification::Updated(b))) => Some(b),
            _ => None,
        }
    });
    tokio::time::sleep(Duration::from_secs(2)).await;

    let sam_tile = tile(&sam, &params, "sam: also new here", 1_785_527_500_000);
    let upd = demux.expect(id, ReplyKind::Update);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Update {
            key,
            data: UpdateData::Delta(StateDelta::from(cbor(&vec![sam_tile]))),
        }))
        .await?;
    tokio::time::timeout(Duration::from_secs(120), upd).await??;

    let Some(bytes) = watcher.await? else {
        println!("no notification arrived");
        return Ok(());
    };
    let grid: CellState = ciborium::de::from_reader(&bytes[..])?;
    println!("\nalex's grid refreshed, live — {} tiles:", grid.records.len());
    for r in grid.records.values() {
        verify_self_contained(r, &params)?;
        println!("  ✓ \"{}\" (verified)", r.headline);
    }

    // --- 4. what a scraper of this cell would learn --------------------
    let mut leaked = false;
    for who in [&alex, &sam] {
        let durable = who.verifying_key_bytes();
        if bytes.windows(durable.len()).any(|w| w == durable.as_slice()) {
            leaked = true;
        }
    }
    println!(
        "\nscraper's view: {} tiles, durable identities present: {}",
        grid.records.len(),
        leaked
    );
    println!("  -> a scraper cannot reach either profile from these bytes");

    // --- 5. sam chooses to reveal a profile ----------------------------
    // Only now does sam's durable key enter the picture, and only because
    // sam decided to publish a profile addressed by it.
    let profile_params = sam.profile_params();
    let profile_state = sam.sign_profile(ProfileBody {
        display_name: "sam".into(),
        bio: "revealed only after we matched".into(),
        tags: vec!["p2p".into()],
        photos: vec![],
        thumbnail: vec![2u8; 64],
        encryption_key: None,
        sequence: 1,
    })?;
    let pp_bytes = cbor(&profile_params);
    let pkey = FreenetClient::key_for(&profile_code, &pp_bytes);
    let pput = demux.expect(*pkey.id(), ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&profile_code, &pp_bytes),
            state: WrappedState::new(cbor(&profile_state)),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    tokio::time::timeout(Duration::from_secs(120), pput).await??;
    println!("\nsam revealed a profile at {} (handle {})", pkey, profile_params.handle());

    // Alex, having been given sam's durable key during the match, fetches
    // and verifies it.
    let g = demux.expect(*pkey.id(), ReplyKind::Get);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Get {
            key: pkey.into(),
            return_contract_code: false,
            subscribe: false,
            blocking_subscribe: false,
        }))
        .await?;
    if let lkng_transport_freenet::demux::Reply::Get(pbytes) =
        tokio::time::timeout(Duration::from_secs(120), g).await??
    {
        let fetched: lkng_profile::ProfileState = ciborium::de::from_reader(&pbytes[..])?;
        lkng_identity::verify_profile(&fetched, &profile_params)?;
        let body = fetched.body.as_ref().expect("body");
        println!(
            "alex fetched and verified it: \"{}\" — {}",
            body.display_name, body.bio
        );
    }

    println!("\n--- two strangers met over Freenet, with no server anywhere ---");
    Ok(())
}
