//! Messaging a stranger from nothing but their grid tile.
//!
//! `first_message` proves the path where someone *shares a profile* first.
//! This proves the one the app actually uses most: you see a face in the
//! grid, you tap message, and it works — no profile exchanged, no handshake,
//! nothing agreed in advance.
//!
//! What is being demonstrated, and why each step is here:
//!
//! 1. Sam posts a tile. It carries an X25519 key derived from Sam's
//!    **epoch** identity, not the durable one.
//! 2. Alex reads the cell off the network, exactly as a scraper would, and
//!    verifies the record. Everything Alex uses from here comes from those
//!    bytes and nowhere else.
//! 3. Alex seals a message using only the tile, and writes it into the
//!    inbox that tile addresses.
//! 4. Sam reads it.
//! 5. **The tile does not leak Sam's durable identity** — asserted against
//!    the actual bytes, not by inspection. This is the property the whole
//!    epoch-key design exists to protect, and it is the one that would fail
//!    silently if someone later "simplified" the encryption key to a durable
//!    one: messaging would still work perfectly, and every user would become
//!    permanently trackable.
//!
//! Usage: tile_to_message <cell.wasm> <cell_params.bin> <inbox.wasm>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{RelatedContracts, StateDelta, UpdateData, WrappedState};
use lkng_identity::Identity;
use lkng_inbox::{InboxParams, InboxState};
use lkng_presence::{verify::verify_self_contained, CellParams, CellState, PresenceRecord};
use lkng_transport_freenet::demux::{Demux, Reply, ReplyKind};
use lkng_transport_freenet::{FreenetClient, DEFAULT_NODE_URL};

fn cbor(v: &impl serde::Serialize) -> Vec<u8> {
    let mut b = Vec::new();
    ciborium::ser::into_writer(v, &mut b).expect("cbor");
    b
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cell_code = std::fs::read(&a[0])?;
    let cell_params_bytes = std::fs::read(&a[1])?;
    let inbox_code = std::fs::read(&a[2])?;
    let params: CellParams = ciborium::de::from_reader(&cell_params_bytes[..])?;

    let sam = Identity::from_seed([0x51; 32]);
    let alex = Identity::from_seed([0xA7; 32]);

    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));

    // --- Sam posts a tile, and an empty inbox to receive replies ---------
    let mut rec = PresenceRecord {
        pseudonym: [0; 32],
        headline: "up late, bad films".into(),
        thumbnail: vec![7u8; 64],
        timestamp_ms: 1_785_632_000_000,
        position: 0,
        age_band: 0,
        verifying_key: None,
        encryption_key: None,
        writer_cert: None,
        sig: vec![],
    };
    sam.sign_presence(&mut rec, &params)?;
    verify_self_contained(&rec, &params)?;

    let mut cell = CellState::default();
    cell.insert(rec.clone());
    let cp = cbor(&params);
    let ckey = FreenetClient::key_for(&cell_code, &cp);
    let r = demux.expect(*ckey.id(), ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&cell_code, &cp),
            state: WrappedState::new(cbor(&cell)),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;
    println!("sam's tile is in cell {}: {ckey}", params.cell_id);

    // Sam's inbox is addressed by Sam's **epoch** key, because that is the
    // only key a stranger holding the tile can possibly know. Addressing it
    // by the durable key instead is not a style choice — it fails on the
    // network with "signature verification failed", because the envelope is
    // bound to one key while the contract is addressed by another. Sam
    // watches the current and previous epoch, exactly as the grid does.
    let sam_inbox = InboxParams::new(sam.for_epoch(params.epoch).verifying_key_bytes());
    let ip = cbor(&sam_inbox);
    let ikey = FreenetClient::key_for(&inbox_code, &ip);
    let r = demux.expect(*ikey.id(), ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&inbox_code, &ip),
            state: WrappedState::new(cbor(&InboxState::default())),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;
    println!("sam's inbox published:    {ikey}");

    // --- Alex reads the cell, seeing exactly what any peer would ---------
    let g = demux.expect(*ckey.id(), ReplyKind::Get);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Get {
            key: ckey.into(),
            return_contract_code: false,
            subscribe: false,
            blocking_subscribe: false,
        }))
        .await?;
    let Reply::Get(cell_bytes) = demux.await_reply(g, Duration::from_secs(120)).await? else {
        unreachable!()
    };
    let fetched: CellState = ciborium::de::from_reader(&cell_bytes[..])?;
    let theirs = fetched
        .records
        .values()
        .find(|r| r.pseudonym == rec.pseudonym)
        .expect("sam's tile should be in the cell");
    verify_self_contained(theirs, &params)?;
    println!("\nalex fetched and verified a tile: \"{}\"", theirs.headline);

    // --- The privacy assertion, against the bytes that crossed the wire --
    //
    // Checked before the message is sent, so a failure here stops the run
    // rather than being buried under a success message.
    let durable_vk = sam.verifying_key_bytes();
    assert!(
        !cell_bytes
            .windows(durable_vk.len())
            .any(|w| w == durable_vk.as_slice()),
        "durable verifying key must never appear in public cell state"
    );
    let durable_enc = sam.encryption_public_key();
    assert!(
        !cell_bytes.windows(32).any(|w| w == durable_enc),
        "durable encryption key must never appear in public cell state"
    );
    assert_ne!(
        theirs.encryption_key.as_deref(),
        Some(&durable_enc[..]),
        "the tile's encryption key must be the epoch key, not the durable one"
    );
    println!("  tile leaks neither durable signing key nor durable encryption key");

    // --- Alex writes, using ONLY what the tile carried -------------------
    let enc: [u8; 32] = theirs.encryption_key.as_deref().expect("reachable").try_into()?;
    let their_vk = theirs.verifying_key.clone().expect("self-contained");
    let env = alex.seal_message(
        &enc,
        &their_vk,
        params.epoch,
        b"bad films are the best films. tonight?",
        1_785_632_100_000,
    )?;

    // Seed-then-update: to write to a contract this node does not host, the
    // code and a starting state must travel on the same session. An empty
    // inbox cannot clobber a real one — the state is a commutative merge.
    let mut delta = InboxState::default();
    delta.insert(env);
    let r = demux.expect(*ikey.id(), ReplyKind::Update);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Update {
            key: ikey,
            data: UpdateData::Delta(StateDelta::from(cbor(&delta))),
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;
    println!("alex sealed a message using only the tile, and sent it");

    // --- Sam reads it ----------------------------------------------------
    let g = demux.expect(*ikey.id(), ReplyKind::Get);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Get {
            key: ikey.into(),
            return_contract_code: false,
            subscribe: false,
            blocking_subscribe: false,
        }))
        .await?;
    let Reply::Get(inbox_bytes) = demux.await_reply(g, Duration::from_secs(120)).await? else {
        unreachable!()
    };
    let inbox: InboxState = ciborium::de::from_reader(&inbox_bytes[..])?;
    lkng_inbox::verify::verify_state(&inbox, &sam_inbox)?;

    let mut read = 0usize;
    for env in inbox.pending() {
        // Opened with the *epoch* identity: the envelope was sealed to the
        // epoch encryption key that the tile advertised, so the durable
        // identity cannot open it -- which is the property, not a limitation.
        let Ok(text) = sam.for_epoch(params.epoch).open_message(env) else {
            continue;
        };
        println!("\nsam reads: \"{}\"", String::from_utf8_lossy(&text));
        // Nobody else can read it, and the plaintext never crossed the wire.
        assert!(Identity::from_seed([0xEE; 32]).open_message(env).is_err());
        assert!(!inbox_bytes.windows(text.len()).any(|w| w == text.as_slice()));
        read += 1;
    }
    assert!(read > 0, "sam must be able to read the message alex sent");

    println!("\n--- tile → message → read, with no profile ever exchanged ---");
    demux.close().await;
    Ok(())
}
