//! The complete social loop on the live network.
//!
//! Alex sees Sam's tile in the grid, Sam shares a profile, Alex writes to
//! Sam using nothing but that profile, and Sam reads it. Every hop is real
//! network traffic; nothing is stubbed.
//!
//! Usage: first_message <profile.wasm> <inbox.wasm>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{RelatedContracts, WrappedState};
use lkng_identity::Identity;
use lkng_inbox::InboxState;
use lkng_profile::{ProfileBody, ProfileState};
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
    let profile_code = std::fs::read(&a[0])?;
    let inbox_code = std::fs::read(&a[1])?;

    // Two people who met through a grid tile a moment ago.
    let sam = Identity::from_seed([0x5A; 32]);
    let alex = Identity::from_seed([0xA1; 32]);

    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));

    // --- Sam publishes a profile, and an empty inbox to receive replies --
    let sam_profile_params = sam.profile_params();
    let sam_profile = sam.sign_profile(ProfileBody {
        display_name: "sam".into(),
        bio: "say hi if you like bad films".into(),
        tags: vec!["films".into()],
        photos: vec![],
        thumbnail: vec![3u8; 64],
        encryption_key: None, // filled in by sign_profile — see its doc
        sequence: 1,
    })?;
    let pp = cbor(&sam_profile_params);
    let pkey = FreenetClient::key_for(&profile_code, &pp);
    let r = demux.expect(*pkey.id(), ReplyKind::Put);
    demux.send(ClientRequest::ContractOp(ContractRequest::Put {
        contract: FreenetClient::container(&profile_code, &pp),
        state: WrappedState::new(cbor(&sam_profile)),
        related_contracts: RelatedContracts::default(),
        subscribe: true,
        blocking_subscribe: false,
    })).await?;
    tokio::time::timeout(Duration::from_secs(120), r).await??;
    println!("sam's profile published: {pkey}");

    let sam_inbox_params = sam.inbox_params();
    let ip = cbor(&sam_inbox_params);
    let ikey = FreenetClient::key_for(&inbox_code, &ip);
    let r = demux.expect(*ikey.id(), ReplyKind::Put);
    demux.send(ClientRequest::ContractOp(ContractRequest::Put {
        contract: FreenetClient::container(&inbox_code, &ip),
        state: WrappedState::new(cbor(&InboxState::default())),
        related_contracts: RelatedContracts::default(),
        subscribe: true,
        blocking_subscribe: false,
    })).await?;
    tokio::time::timeout(Duration::from_secs(120), r).await??;
    println!("sam's inbox published:   {ikey}");

    // --- Alex fetches the profile from the network -----------------------
    // This is all alex has: an address sam chose to share.
    let g = demux.expect(*pkey.id(), ReplyKind::Get);
    demux.send(ClientRequest::ContractOp(ContractRequest::Get {
        key: pkey.into(),
        return_contract_code: false,
        subscribe: false,
        blocking_subscribe: false,
    })).await?;
    let Reply::Get(bytes) = tokio::time::timeout(Duration::from_secs(120), g).await??
    else { unreachable!() };

    let fetched: ProfileState = ciborium::de::from_reader(&bytes[..])?;
    lkng_identity::verify_profile(&fetched, &sam_profile_params)?;
    let body = fetched.body.as_ref().expect("body");
    println!("\nalex fetched and verified: \"{}\" — {}", body.display_name, body.bio);

    // --- Alex writes, using only what the profile gave them --------------
    let enc: [u8; 32] = body.encryption_key.as_ref().expect("reachable")[..].try_into()?;
    let env = alex.seal_message(
        &enc,
        &sam_profile_params.owner_vk,
        20674,
        b"the worse the film the better. friday?",
        1_785_531_000_000,
    )?;
    let mut delta = InboxState::default();
    delta.insert(env);

    let r = demux.expect(*ikey.id(), ReplyKind::Update);
    demux.send(ClientRequest::ContractOp(ContractRequest::Update {
        key: ikey,
        data: freenet_stdlib::prelude::UpdateData::Delta(
            freenet_stdlib::prelude::StateDelta::from(cbor(&delta)),
        ),
    })).await?;
    tokio::time::timeout(Duration::from_secs(120), r).await??;
    println!("alex sealed a message to sam's inbox and sent it");

    // --- Sam reads it back off the network -------------------------------
    let g = demux.expect(*ikey.id(), ReplyKind::Get);
    demux.send(ClientRequest::ContractOp(ContractRequest::Get {
        key: ikey.into(),
        return_contract_code: false,
        subscribe: false,
        blocking_subscribe: false,
    })).await?;
    let Reply::Get(inbox_bytes) = tokio::time::timeout(Duration::from_secs(120), g).await??
    else { unreachable!() };

    let inbox: InboxState = ciborium::de::from_reader(&inbox_bytes[..])?;
    lkng_inbox::verify::verify_state(&inbox, &sam_inbox_params)?;
    println!("\nsam's inbox: {} pending", inbox.pending().len());
    for env in inbox.pending() {
        let text = sam.open_message(env)?;
        println!("  \"{}\"", String::from_utf8_lossy(&text));
        // The message alex sent is unreadable to anyone else, and its
        // plaintext never appears in what crossed the wire.
        assert!(Identity::from_seed([0xE5; 32]).open_message(env).is_err());
        assert!(!inbox_bytes.windows(text.len()).any(|w| w == text.as_slice()));
    }

    println!("\n--- profile → message → read, all over Freenet, no server ---");
    Ok(())
}
