use lkng_presence::{CellState, PresenceRecord};
fn main() {
    // Full state carrying ONLY the second record — update_state merges
    // (union), so sending it --as-state must yield both records. This is
    // the commutative-monoid property doing real work on the network.
    let mut cell = CellState::default();
    cell.insert(PresenceRecord {
        pseudonym: *blake3::hash(b"lkng-second-user").as_bytes(),
        headline: "second tile — merged over the network".into(),
        thumbnail: vec![7u8; 64],
        timestamp_ms: 1_785_523_000_000,
        verifying_key: None,
        writer_cert: None,
        sig: vec![2u8; 64],
    });
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&cell, &mut buf).unwrap();
    std::fs::write("state2.bin", &buf).unwrap();
    println!("state2: {} bytes", buf.len());
}
