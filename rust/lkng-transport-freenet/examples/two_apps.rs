//! Two app instances publishing into one cell, exactly as the app does it.
//!
//! `two_strangers` proved discovery using a single PUT of a full cell state.
//! The app does not do that. It does **seed-then-update**: a PUT carrying an
//! empty-ish starting state so the contract exists locally, immediately
//! followed by a delta. That is a different code path, and the difference is
//! not cosmetic — the app's entire presence feature runs on the untested one.
//!
//! It also matters that *both* writers arrive this way. A cell where one
//! participant seeded and another updated is the common case in the real
//! world (someone is always second), and it is where an ordering assumption
//! would show up.
//!
//! What is asserted:
//!
//! 1. both records survive the merge — the second writer does not clobber
//!    the first, which is the failure a naive "seed with my state" would
//!    produce and which would look, to the user, like nobody else is around;
//! 2. each side verifies the other's record from the fetched bytes;
//! 3. neither durable identity appears anywhere in the cell.
//!
//! Usage: two_apps <cell.wasm> <cell_params.bin>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{RelatedContracts, StateDelta, UpdateData, WrappedState};
use lkng_app::{Session, TileFilters};
use lkng_identity::Identity;
use lkng_presence::{verify::verify_self_contained, CellParams, CellState};
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
    let params: CellParams = ciborium::de::from_reader(&std::fs::read(&a[1])?[..])?;

    let params_bytes = cbor(&params);
    let key = FreenetClient::key_for(&cell_code, &params_bytes);

    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));

    // Two installs, each with its own seed, as two phones would have.
    let people = [
        (Identity::from_seed([0xC1; 32]), "first one here", 3u8, 2u8),
        (Identity::from_seed([0xC2; 32]), "second one here", 4u8, 5u8),
    ];

    let mut mine = Vec::new();
    for (id, headline, age_band, position) in people {
        let session = Session::new(id, [0x77; 32], lkng_app::Privacy::Km1);
        let rec = session.compose_tile_with(
            &params,
            headline,
            vec![0x40; 64],
            1_785_633_000_000,
            TileFilters { age_band, position },
        )?;

        // Exactly the app's sequence. The seed carries only *our* record: we
        // have no business asserting anything about what else is in the cell,
        // and a seed built from a stale local view is how one client silently
        // rolls back another's write.
        let mut state = CellState::default();
        state.insert(rec.clone());

        // Seed only if the cell is genuinely new to us. A PUT against a
        // contract that already exists is not a harmless no-op here: it can
        // time out, and a timed-out PUT takes the whole session down with
        // it. The app has the same shape, which is why `Node::seed_once`
        // exists -- an unconditional seed on a republish timer is a full
        // contract container pushed into the network every few minutes.
        let g = demux.expect(*key.id(), ReplyKind::Get);
        demux
            .send(ClientRequest::ContractOp(ContractRequest::Get {
                key: key.clone().into(),
                return_contract_code: false,
                subscribe: false,
                blocking_subscribe: false,
            }))
            .await?;
        let exists = demux.await_reply(g, Duration::from_secs(60)).await.is_ok();

        if !exists {
            let r = demux.expect(*key.id(), ReplyKind::Put);
            demux
                .send(ClientRequest::ContractOp(ContractRequest::Put {
                    contract: FreenetClient::container(&cell_code, &params_bytes),
                    state: WrappedState::new(cbor(&state)),
                    related_contracts: RelatedContracts::default(),
                    subscribe: true,
                    blocking_subscribe: false,
                }))
                .await?;
            demux.await_reply(r, Duration::from_secs(120)).await?;
            println!("  seeded the cell (it did not exist yet)");
        }

        let r = demux.expect(*key.id(), ReplyKind::Update);
        demux
            .send(ClientRequest::ContractOp(ContractRequest::Update {
                key: key.clone(),
                // A list of records, which is what the contract decodes.
                // Sending the CellState map instead fails with
                // "invalid type: map, expected array" -- the bug this
                // example was written to catch, and did.
                data: UpdateData::Delta(StateDelta::from(session.tile_delta(&rec)?)),
            }))
            .await?;
        demux.await_reply(r, Duration::from_secs(120)).await?;
        println!("published: \"{headline}\"");
        mine.push(rec);
    }

    // --- Read the cell back, as either phone's grid would ----------------
    let g = demux.expect(*key.id(), ReplyKind::Get);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Get {
            key: key.clone().into(),
            return_contract_code: false,
            subscribe: false,
            blocking_subscribe: false,
        }))
        .await?;
    let Reply::Get(bytes) = demux.await_reply(g, Duration::from_secs(120)).await? else {
        unreachable!()
    };
    let cell: CellState = ciborium::de::from_reader(&bytes[..])?;
    println!("\ncell holds {} record(s), {} bytes", cell.records.len(), bytes.len());

    // 1. Both survived. This is the assertion with teeth: if the second
    //    writer's seed had replaced the state rather than merged into it,
    //    exactly one record would be here and the app would look empty to
    //    everyone but the last person to open it.
    for rec in &mine {
        let found = cell
            .records
            .values()
            .find(|r| r.pseudonym == rec.pseudonym)
            .unwrap_or_else(|| {
                panic!("record \"{}\" was lost in the merge", rec.headline)
            });
        // 2. Verified from the fetched bytes, not from what we sent.
        verify_self_contained(found, &params)?;
        assert!(
            found.encryption_key.is_some(),
            "a tile without an encryption key cannot be messaged"
        );
        println!("  verified: \"{}\"", found.headline);
    }

    // 3. No durable identity anywhere in the cell.
    for (id, ..) in [
        (Identity::from_seed([0xC1; 32]), 0),
        (Identity::from_seed([0xC2; 32]), 0),
    ] {
        let vk = id.verifying_key_bytes();
        assert!(
            !bytes.windows(vk.len()).any(|w| w == vk.as_slice()),
            "a durable verifying key reached public cell state"
        );
        let enc = id.encryption_public_key();
        assert!(
            !bytes.windows(32).any(|w| w == enc),
            "a durable encryption key reached public cell state"
        );
    }
    println!("  no durable identity in {} bytes of cell state", bytes.len());

    println!("\n--- two installs, seed-then-update, both discoverable ---");
    demux.close().await;
    Ok(())
}
