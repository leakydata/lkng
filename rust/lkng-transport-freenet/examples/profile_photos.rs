//! A profile with several photos, published and read back off the network.
//!
//! Written because the two previous photo features were both broken in ways
//! reading them could not reveal: presence published a delta the contract
//! rejected, and the profile editor made no network write at all. Photos
//! looked finished twice and were device-local both times.
//!
//! What is asserted against bytes fetched back:
//!
//! 1. every photo survives the round trip byte-for-byte;
//! 2. **the primary is the one the owner chose**, not the first in the list —
//!    the field that decides which face strangers see;
//! 3. changing the primary and republishing changes what a reader sees;
//! 4. a tampered profile fails verification, so a peer cannot swap someone's
//!    main photo for another.
//!
//! Usage: profile_photos <profile.wasm>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{RelatedContracts, StateDelta, UpdateData, WrappedState};
use lkng_identity::Identity;
use lkng_profile::{PhotoRef, ProfileBody, ProfileParams, ProfileState};
use lkng_transport_freenet::demux::{Demux, Reply, ReplyKind};
use lkng_transport_freenet::{FreenetClient, DEFAULT_NODE_URL};

fn cbor(v: &impl serde::Serialize) -> Vec<u8> {
    let mut b = Vec::new();
    ciborium::ser::into_writer(v, &mut b).expect("cbor");
    b
}

/// Distinct, recognisable bytes per photo, so "which one came back" is
/// answerable rather than inferred.
fn photo_bytes(tag: u8) -> Vec<u8> {
    (0..4096u32).map(|i| (i as u8) ^ tag).collect()
}

fn body(seq: u64, primary: usize) -> ProfileBody {
    ProfileBody {
        display_name: "sam".into(),
        bio: "bad films, good coffee".into(),
        tags: vec![],
        photos: (0..3u8)
            .map(|i| PhotoRef::new(photo_bytes(i + 1), i as usize == primary))
            .collect(),
        thumbnail: vec![7; 512],
        demographics: Default::default(),
        encryption_key: None,
        sequence: seq,
    }
}

async fn publish(
    demux: &Demux,
    code: &[u8],
    params_bytes: &[u8],
    key: &freenet_stdlib::prelude::ContractKey,
    state: &ProfileState,
    first: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if first {
        let r = demux.expect(*key.id(), ReplyKind::Put);
        demux
            .send(ClientRequest::ContractOp(ContractRequest::Put {
                contract: FreenetClient::container(code, params_bytes),
                state: WrappedState::new(cbor(state)),
                related_contracts: RelatedContracts::default(),
                subscribe: true,
                blocking_subscribe: false,
            }))
            .await?;
        demux.await_reply(r, Duration::from_secs(120)).await?;
    } else {
        let r = demux.expect(*key.id(), ReplyKind::Update);
        demux
            .send(ClientRequest::ContractOp(ContractRequest::Update {
                key: key.clone(),
                data: UpdateData::Delta(StateDelta::from(cbor(state))),
            }))
            .await?;
        demux.await_reply(r, Duration::from_secs(120)).await?;
    }
    Ok(())
}

async fn fetch(
    demux: &Demux,
    key: &freenet_stdlib::prelude::ContractKey,
) -> Result<(ProfileState, Vec<u8>), Box<dyn std::error::Error>> {
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
    let state: ProfileState = ciborium::de::from_reader(&bytes[..])?;
    Ok((state, bytes))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let code = std::fs::read(&a[0])?;

    let owner = Identity::from_seed([0x8C; 32]);
    let params: ProfileParams = owner.profile_params();
    let params_bytes = cbor(&params);
    let key = FreenetClient::key_for(&code, &params_bytes);

    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));

    // --- Publish with the SECOND photo as primary ------------------------
    //
    // Deliberately not the first: if the primary flag were ignored and the
    // reader just took photos[0], every assertion below would still pass
    // with primary = 0 and the bug would ship.
    let state = owner.sign_profile(body(1, 1))?;
    publish(&demux, &code, &params_bytes, &key, &state, true).await?;
    println!("published a profile with 3 photos, primary = #2: {key}");

    // --- Read it back ----------------------------------------------------
    let (fetched, raw) = fetch(&demux, &key).await?;
    lkng_identity::verify_profile(&fetched, &params)?;
    let got = fetched.body.as_ref().expect("body");
    println!("\nfetched and verified: {} bytes", raw.len());

    // 1. Photos survive byte-for-byte.
    assert_eq!(got.photos.len(), 3, "all three photos should be present");
    for (i, p) in got.photos.iter().enumerate() {
        assert_eq!(
            p.bytes,
            photo_bytes(i as u8 + 1),
            "photo #{} came back different from what was published",
            i + 1
        );
    }
    println!("  all 3 photos identical to what was published");

    // 2. The owner's choice of primary survived.
    let primary: Vec<usize> = got
        .photos
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_primary)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(primary, vec![1], "primary must be photo #2, exactly one of them");
    println!("  primary is photo #2, as chosen -- not defaulted to the first");

    // --- 3. Change the primary and republish -----------------------------
    let state2 = owner.sign_profile(body(2, 2))?;
    publish(&demux, &code, &params_bytes, &key, &state2, false).await?;
    let (after, _) = fetch(&demux, &key).await?;
    lkng_identity::verify_profile(&after, &params)?;
    let got2 = after.body.as_ref().expect("body");
    let primary2: Vec<usize> = got2
        .photos
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_primary)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(primary2, vec![2], "changing the primary must be visible to readers");
    println!("  changed primary to photo #3 and a reader sees it");

    // --- 4. A tampered primary must not verify ---------------------------
    //
    // The attack this prevents: a peer flips whose face a profile leads with
    // and everything else about it stays valid.
    let mut tampered = after.clone();
    if let Some(b) = tampered.body.as_mut() {
        for (i, p) in b.photos.iter_mut().enumerate() {
            p.is_primary = i == 0;
        }
    }
    assert!(
        lkng_identity::verify_profile(&tampered, &params).is_err(),
        "swapping which photo is primary must break the signature"
    );
    println!("  and a swapped primary fails verification");

    println!("\n--- profile photos: published, read back, owner's choice intact ---");
    demux.close().await;
    Ok(())
}
