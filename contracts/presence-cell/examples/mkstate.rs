use lkng_presence::{CellState, PresenceRecord};
fn main() {
    let mut cell = CellState::default();
    cell.insert(PresenceRecord {
        pseudonym: *blake3::hash(b"lkng-genesis-pseudonym").as_bytes(),
        headline: "first tile in the grid".into(),
        thumbnail: vec![0u8; 64], // placeholder tile
        timestamp_ms: 1_785_522_000_000,
        writer_cert: None,
        sig: vec![1u8; 64], // placeholder sig (real ML-DSA lands with identity delegate)
    });
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&cell, &mut buf).unwrap();
    std::fs::write("initial_state.bin", &buf).unwrap();
    println!("state: {} bytes, {} record(s)", buf.len(), cell.records.len());
}
