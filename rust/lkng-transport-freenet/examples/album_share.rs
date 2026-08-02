//! Sharing a private album, on the live network.
//!
//! The claim being tested is the one the whole album design rests on: the
//! photos are on a public network, anyone can fetch them, and only the
//! people the owner named can read them.
//!
//! Written before any UI, because every write path built tonight without
//! running it was broken — twice by a delta encoded in a shape the contract
//! does not decode, and once by a doc comment the network refuted.
//!
//! What is asserted, against bytes fetched back off the network:
//!
//! 1. a stranger who fetches the album gets ciphertext and cannot read it;
//! 2. a grantee, holding only what arrived in their inbox, can;
//! 3. the plaintext never appears anywhere in the stored bytes;
//! 4. after revocation the removed viewer keeps the old photos and cannot
//!    read the new one — prospective revocation, on real data.
//!
//! Usage: album_share <album.wasm> <inbox.wasm>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{RelatedContracts, StateDelta, UpdateData, WrappedState};
use lkng_album::{address_of, AlbumParams, AlbumState, Grant};
use lkng_identity::Identity;
use lkng_inbox::{InboxParams, InboxState};
use lkng_transport_freenet::demux::{Demux, Reply, ReplyKind};
use lkng_transport_freenet::{FreenetClient, DEFAULT_NODE_URL};

fn cbor(v: &impl serde::Serialize) -> Vec<u8> {
    let mut b = Vec::new();
    ciborium::ser::into_writer(v, &mut b).expect("cbor");
    b
}

const SECRET_A: &[u8] = b"private-photo-one-plaintext";
const SECRET_B: &[u8] = b"private-photo-two-plaintext";
const SECRET_C: &[u8] = b"added-after-revocation-plaintext";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let album_code = std::fs::read(&a[0])?;
    let inbox_code = std::fs::read(&a[1])?;

    let owner = Identity::from_seed([0x71; 32]);
    let friend = Identity::from_seed([0x72; 32]);
    let stranger = Identity::from_seed([0x73; 32]);

    let params = AlbumParams {
        schema_v: 1,
        address: address_of(&owner.verifying_key_bytes(), 0),
    };
    let params_bytes = cbor(&params);
    let key = FreenetClient::key_for(&album_code, &params_bytes);

    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));

    // --- The owner builds and publishes an album -------------------------
    let album_key = [0x5A; 32];
    let mut album = AlbumState { generation: 1, ..Default::default() };
    for (i, secret) in [SECRET_A, SECRET_B].iter().enumerate() {
        let mut p = Identity::seal_album_photo(&album_key, secret, [i as u8 + 1; 24])?;
        p.generation = 1;
        p.added_ms = 1_785_636_000_000 + i as u64;
        album.insert(p);
    }
    owner.sign_album(&mut album, &params)?;

    let r = demux.expect(*key.id(), ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&album_code, &params_bytes),
            state: WrappedState::new(cbor(&album)),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;
    println!("album published (2 photos, generation 1): {key}");

    // --- The owner grants the friend access, through the inbox -----------
    let grant = Grant {
        address: params.address,
        key: album_key.to_vec(),
        generation: 1,
        owner_vk: owner.verifying_key_bytes(),
    };
    let friend_inbox = InboxParams::new(friend.for_epoch(2000).verifying_key_bytes());
    let ip = cbor(&friend_inbox);
    let ikey = FreenetClient::key_for(&inbox_code, &ip);

    let env = owner.seal_message(
        &friend.for_epoch(2000).encryption_public_key(),
        &friend.for_epoch(2000).verifying_key_bytes(),
        2000,
        &grant.encode()?,
        1_785_636_100_000,
    )?;
    let mut seeded = InboxState::default();
    seeded.insert(env);

    let r = demux.expect(*ikey.id(), ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&inbox_code, &ip),
            state: WrappedState::new(cbor(&seeded)),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;
    println!("grant sent to the friend's inbox: {ikey}");

    // --- Everyone fetches the album. It is public. -----------------------
    let g = demux.expect(*key.id(), ReplyKind::Get);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Get {
            key: key.clone().into(),
            return_contract_code: false,
            subscribe: false,
            blocking_subscribe: false,
        }))
        .await?;
    let Reply::Get(album_bytes) = demux.await_reply(g, Duration::from_secs(120)).await? else {
        unreachable!()
    };
    let fetched: AlbumState = ciborium::de::from_reader(&album_bytes[..])?;
    lkng_album::verify::verify_album(&fetched, &params)?;
    println!("\nanyone can fetch it: {} bytes, verified", album_bytes.len());

    // 3. No plaintext anywhere in what crossed the wire.
    for secret in [SECRET_A, SECRET_B] {
        assert!(
            !album_bytes.windows(secret.len()).any(|w| w == secret),
            "a photo's plaintext reached the network"
        );
    }
    println!("  and none of the plaintext is in those bytes");

    // 1. The stranger has the ciphertext and no way in. They can guess keys
    //    forever; this checks the one they would actually try.
    let some_photo = fetched.photos.values().next().expect("a photo");
    assert!(
        Identity::open_album_photo(&[0u8; 32], some_photo).is_err(),
        "a stranger must not be able to open an album photo"
    );
    let _ = &stranger;
    println!("  a stranger holding the album cannot read it");

    // 2. The friend reads their inbox and gets in with what they find there.
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

    let mut recovered: Option<Grant> = None;
    for env in inbox.envelopes.values() {
        if let Ok(plain) = friend.for_epoch(2000).open_message(env) {
            if let Some(g) = Grant::decode(&plain) {
                recovered = Some(g);
            }
        }
    }
    let grant = recovered.expect("the friend should find a grant in their inbox");
    assert_eq!(grant.owner_vk, owner.verifying_key_bytes());
    let friend_key: [u8; 32] = grant.key[..].try_into()?;

    let mut read = Vec::new();
    for p in fetched.readable_at(grant.generation) {
        read.push(Identity::open_album_photo(&friend_key, p)?);
    }
    assert_eq!(read.len(), 2, "the friend should read both photos");
    assert!(read.iter().any(|r| r == SECRET_A));
    println!("  the friend, using only what was in their inbox, reads both photos");

    // --- Revocation: new key, new generation, one new photo --------------
    let new_key = [0x6B; 32];
    let mut album2 = AlbumState { generation: 2, ..Default::default() };
    for p in fetched.photos.values() {
        album2.insert(p.clone()); // old photos stay as they were
    }
    let mut fresh = Identity::seal_album_photo(&new_key, SECRET_C, [9; 24])?;
    fresh.generation = 2;
    fresh.added_ms = 1_785_636_200_000;
    album2.insert(fresh);
    owner.sign_album(&mut album2, &params)?;

    let r = demux.expect(*key.id(), ReplyKind::Update);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Update {
            key: key.clone(),
            data: UpdateData::Delta(StateDelta::from(cbor(&album2))),
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;

    let g = demux.expect(*key.id(), ReplyKind::Get);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Get {
            key: key.into(),
            return_contract_code: false,
            subscribe: false,
            blocking_subscribe: false,
        }))
        .await?;
    let Reply::Get(after_bytes) = demux.await_reply(g, Duration::from_secs(120)).await? else {
        unreachable!()
    };
    let after: AlbumState = ciborium::de::from_reader(&after_bytes[..])?;
    lkng_album::verify::verify_album(&after, &params)?;
    println!("\nrevoked: album is now generation {}", after.generation);

    // 4. The removed viewer keeps what they had, and gains nothing.
    let still: Vec<Vec<u8>> = after
        .readable_at(1)
        .into_iter()
        .filter_map(|p| Identity::open_album_photo(&friend_key, p).ok())
        .collect();
    assert_eq!(still.len(), 2, "they keep what was already shared with them");

    let newest = after
        .photos
        .values()
        .find(|p| p.generation == 2)
        .expect("the new photo");
    assert!(
        Identity::open_album_photo(&friend_key, newest).is_err(),
        "a removed viewer must not be able to read a photo added afterwards"
    );
    println!("  they keep the 2 old photos and cannot read the new one");
    println!("  (which is what prospective revocation means -- the bytes they");
    println!("   already had are theirs, and no contract can take them back)");

    println!("\n--- private album: public bytes, readable only by the named ---");
    demux.close().await;
    Ok(())
}
