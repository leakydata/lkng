//! Carrying an inbox forward to a new contract address, on the live network.
//!
//! When a contract's code changes, its address changes with it, and every
//! message anyone had received stays at the old one.
//!
//! The first version of this example tried to *move* that mail: fetch the
//! retired address, merge it, publish the result at the new one. The network
//! rejected it immediately —
//!
//! ```text
//! UPDATE failed: inbox failed verification: signature verification failed
//! ```
//!
//! — because an envelope's signature covers the inbox parameters it was
//! addressed to. That binding is what stops anyone lifting an envelope out
//! of one person's inbox and replaying it into another's, so it is a
//! property to keep, not an obstacle.
//!
//! Migration is therefore a **read**: fetch the retired address, decrypt
//! what you can, keep the plaintext on the device — which is already where
//! the user's sent messages live and already authoritative for history. The
//! network delivers; the device archives.
//!
//! Two distinct addresses are produced here by varying the *parameters*
//! rather than the code, which is the same arithmetic —
//! `BLAKE3(BLAKE3(wasm) || params)` — reached from the other side. It keeps
//! the example runnable without shipping two builds of the contract, and
//! exercises exactly the thing that matters: a GET against an address this
//! client no longer uses, merged into the one it does.
//!
//! Usage: migrate_forward <inbox.wasm>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{RelatedContracts, StateDelta, UpdateData, WrappedState};
use lkng_identity::Identity;
use lkng_inbox::{carry_forward, InboxParams, InboxState};
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
    let code = std::fs::read(&a[0])?;

    let owner = Identity::from_seed([0x4D; 32]);
    let sender = Identity::from_seed([0x5E; 32]);

    // "Old" and "new" addresses. Different epoch keys give different inbox
    // parameters, hence different contract ids -- the same situation a code
    // change produces.
    let old_params = InboxParams::new(owner.for_epoch(1000).verifying_key_bytes());
    let new_params = InboxParams::new(owner.for_epoch(1001).verifying_key_bytes());
    let old_bytes = cbor(&old_params);
    let new_bytes = cbor(&new_params);
    let old_key = FreenetClient::key_for(&code, &old_bytes);
    let new_key = FreenetClient::key_for(&code, &new_bytes);
    assert_ne!(old_key, new_key, "the two addresses must differ");

    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));

    // --- Mail arrives at the old address ---------------------------------
    let env = sender.seal_message(
        &owner.for_epoch(1000).encryption_public_key(),
        &owner.for_epoch(1000).verifying_key_bytes(),
        1000,
        b"sent before the upgrade",
        1_785_635_000_000,
    )?;
    let mut old_state = InboxState::default();
    old_state.insert(env);

    let r = demux.expect(*old_key.id(), ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&code, &old_bytes),
            state: WrappedState::new(cbor(&old_state)),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;
    println!("old address holds one message: {old_key}");

    // --- Mail also arrives at the new address ----------------------------
    //
    // This is the case a migration written as a *copy* would destroy: the
    // message received since the upgrade. It has to survive.
    let after = sender.seal_message(
        &owner.for_epoch(1001).encryption_public_key(),
        &owner.for_epoch(1001).verifying_key_bytes(),
        1001,
        b"sent after the upgrade",
        1_785_635_100_000,
    )?;
    let mut new_state = InboxState::default();
    new_state.insert(after);

    let r = demux.expect(*new_key.id(), ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&code, &new_bytes),
            state: WrappedState::new(cbor(&new_state)),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;
    println!("new address holds one message: {new_key}");

    // --- The migration: read the retired address, merge it forward -------
    let g = demux.expect(*old_key.id(), ReplyKind::Get);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Get {
            key: old_key.clone().into(),
            return_contract_code: false,
            subscribe: false,
            blocking_subscribe: false,
        }))
        .await?;
    let Reply::Get(legacy_bytes) = demux.await_reply(g, Duration::from_secs(120)).await? else {
        unreachable!()
    };
    let legacy: InboxState = ciborium::de::from_reader(&legacy_bytes[..])?;

    let mut merged = new_state.clone();
    let carried = carry_forward(&mut merged, &legacy);
    println!("\ncarried {carried} message(s) forward into the local view");

    // Deliberately NOT published. `merged` holds envelopes addressed to the
    // retired inbox, which the current contract will refuse — correctly.
    // This is what a client keeps in local storage and renders from.

    let mut read: Vec<String> = Vec::new();
    for env in merged.envelopes.values() {
        // Either epoch identity may hold the key; try both, as a migrating
        // client would.
        for epoch in [1000u64, 1001] {
            if let Ok(text) = owner.for_epoch(epoch).open_message(env) {
                read.push(String::from_utf8_lossy(&text).to_string());
                break;
            }
        }
    }
    read.sort();
    println!("\nreadable by the owner after migration:");
    for m in &read {
        println!("  \"{m}\"");
    }

    assert!(
        read.iter().any(|m| m == "sent before the upgrade"),
        "the message at the retired address was not recovered"
    );
    assert!(
        read.iter().any(|m| m == "sent after the upgrade"),
        "migration destroyed mail that arrived after the upgrade"
    );

    // And prove the binding that forced this design, rather than asserting it.
    let stray = legacy.envelopes.values().next().expect("legacy mail");
    assert!(
        lkng_inbox::verify::verify_envelope(stray, &new_params).is_err(),
        "retired mail must NOT verify at the new address; if this ever passes, \
         envelopes are no longer bound to their inbox and anyone can replay \
         one into someone else's"
    );
    println!("  and retired mail correctly does not verify at the new address");

    println!("\n--- migration is a read into local storage, not a move ---");
    demux.close().await;
    Ok(())
}
