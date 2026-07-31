use lkng_identity::{verify_self_contained, Identity};
use lkng_presence::{CellParams, CellState, PresenceRecord};

fn main() {
    // Deterministic demo seed. Real identities come from the platform
    // CSPRNG inside the identity delegate — never a fixed seed.
    let id = Identity::from_seed([42u8; 32]);
    let params = CellParams { schema_v: 1, cell_id: "9q8yy".into(), epoch: 20673 };

    let mut record = PresenceRecord {
        pseudonym: [0; 32], // filled in by sign_presence
        headline: "first signed tile in the grid".into(),
        thumbnail: vec![0u8; 64],
        timestamp_ms: 1_785_524_000_000,
        verifying_key: None,
        writer_cert: None,
        sig: vec![],
    };
    id.sign_presence(&mut record, &params).expect("sign");

    // Refuse to publish anything that doesn't verify — checked exactly the
    // way a peer on the network will check it, against the epoch key the
    // record carries rather than our durable identity.
    verify_self_contained(&record, &params).expect("self-verify");
    assert_ne!(
        record.verifying_key.as_deref(),
        Some(id.verifying_key_bytes().as_slice()),
        "durable key must never be published"
    );

    let mut cell = CellState::default();
    cell.insert(record);
    assert_eq!(cell.records.len(), 1, "signed record must satisfy state caps");

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&cell, &mut buf).unwrap();
    std::fs::write("initial_state.bin", &buf).unwrap();
    println!(
        "state: {} bytes | durable handle: {} | epoch handle: {} | sig: {} B",
        buf.len(),
        id.fingerprint(),
        id.for_epoch(params.epoch).fingerprint(),
        cell.records.values().next().unwrap().sig.len()
    );
}
