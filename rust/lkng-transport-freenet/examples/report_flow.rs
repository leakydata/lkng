//! Filing reports into a moderation feed on the live network.
//!
//! Written because two write paths tonight were broken in exactly the way
//! that reading them could not reveal — a delta encoded as a map where the
//! contract decodes a list, accepted by the client, dispatched happily, and
//! rejected inside the contract with nothing surfaced to the user. This
//! exercises the report path end to end so the same class of bug cannot
//! survive here.
//!
//! What is asserted:
//!
//! 1. a signed report lands and comes back verifiable from fetched bytes;
//! 2. **three separate reporters count as three, one reporter filing three
//!    times counts as one** — the property that decides whether a single
//!    determined person can manufacture consensus against someone;
//! 3. the reporters' durable identities never appear in feed state.
//!
//! Usage: report_flow <moderation.wasm>
use std::time::Duration;

use freenet_stdlib::client_api::{ClientRequest, ContractRequest, WebApi};
use freenet_stdlib::prelude::{RelatedContracts, StateDelta, UpdateData, WrappedState};
use lkng_identity::Identity;
use lkng_moderation::{FeedParams, FeedState, Reason, Report};
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

    // A feed name unique to this run, so repeated runs do not accumulate
    // into each other and make the counting assertions meaningless.
    let feed = format!("test-{}", 1_785_634_000u64);
    let params = FeedParams { schema_v: 1, feed };
    let params_bytes = cbor(&params);
    let key = FreenetClient::key_for(&code, &params_bytes);

    let (stream, _) = tokio_tungstenite::connect_async(DEFAULT_NODE_URL).await?;
    let demux = Demux::spawn(WebApi::start(stream));

    let subject = [0xD1u8; 32];
    let epoch = 82_668u64;

    // Three distinct people, plus one of them filing twice more. If reports
    // were counted rather than reporters, this subject would look like it
    // had five accusers instead of three.
    let authors: Vec<Identity> = (0..3)
        .map(|i| Identity::from_seed([0xE0 + i as u8; 32]).for_epoch(epoch))
        .collect();

    let mut filed: Vec<Report> = Vec::new();
    for (i, who) in authors.iter().enumerate() {
        let mut r = Report {
            subject,
            reason: Reason::Harassment.code(),
            note: format!("report {i}"),
            timestamp_ms: 1_785_634_000_000 + i as u64,
            verifying_key: None,
            sig: Vec::new(),
        };
        who.sign_report(&mut r, &params)?;
        filed.push(r);
    }
    for extra in 0..2 {
        let mut r = Report {
            subject,
            reason: Reason::Spam.code(),
            note: format!("again {extra}"),
            timestamp_ms: 1_785_634_100_000 + extra,
            verifying_key: None,
            sig: Vec::new(),
        };
        authors[0].sign_report(&mut r, &params)?;
        filed.push(r);
    }

    // Seed the feed with the first report so the contract exists.
    let mut seed = FeedState::default();
    seed.insert(filed[0].clone());
    let r = demux.expect(*key.id(), ReplyKind::Put);
    demux
        .send(ClientRequest::ContractOp(ContractRequest::Put {
            contract: FreenetClient::container(&code, &params_bytes),
            state: WrappedState::new(cbor(&seed)),
            related_contracts: RelatedContracts::default(),
            subscribe: true,
            blocking_subscribe: false,
        }))
        .await?;
    demux.await_reply(r, Duration::from_secs(120)).await?;
    println!("feed created: {key}");

    // The rest arrive as deltas — **a list of reports**, which is what the
    // contract decodes. A FeedState map here would be rejected inside the
    // contract with the update reported as sent.
    for report in filed.iter().skip(1) {
        let r = demux.expect(*key.id(), ReplyKind::Update);
        demux
            .send(ClientRequest::ContractOp(ContractRequest::Update {
                key: key.clone(),
                data: UpdateData::Delta(StateDelta::from(cbor(&vec![report.clone()]))),
            }))
            .await?;
        demux.await_reply(r, Duration::from_secs(120)).await?;
    }
    println!("filed {} reports from {} people", filed.len(), authors.len());

    // --- Read the feed back ----------------------------------------------
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
    let state: FeedState = ciborium::de::from_reader(&bytes[..])?;
    println!("\nfeed holds {} report(s), {} bytes", state.reports.len(), bytes.len());

    // 1. Verifiable from what came back, not from what we sent.
    for r in state.reports.values() {
        lkng_moderation::verify::verify_report(r, &params)?;
    }
    println!("  all reports verified against the feed");

    // 2. The assertion that matters.
    let reporters = state.reporter_count(&subject);
    assert_eq!(
        reporters, 3,
        "five reports from three people must count as three reporters, not five"
    );
    println!("  {} reports, {reporters} distinct reporters", state.reports.len());

    // 3. No durable identity in public feed state.
    for i in 0..3u8 {
        let durable = Identity::from_seed([0xE0 + i; 32]);
        let vk = durable.verifying_key_bytes();
        assert!(
            !bytes.windows(vk.len()).any(|w| w == vk.as_slice()),
            "a reporter's durable key reached public feed state"
        );
    }
    println!("  no reporter's durable identity in the feed");

    println!("\n--- reports filed, verified, and counted by person ---");
    demux.close().await;
    Ok(())
}
