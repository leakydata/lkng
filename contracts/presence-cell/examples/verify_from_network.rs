//! Verify a cell state fetched from the network — the end-to-end check
//! that what the network gave back is cryptographically genuine.
use lkng_presence::{verify::verify_self_contained, CellParams, CellState};
fn main() {
    let path = std::env::args().nth(1).expect("usage: verify_from_network <state.bin>");
    let bytes = std::fs::read(&path).expect("read state");
    let cell: CellState = ciborium::de::from_reader(&bytes[..]).expect("decode state");
    let params = CellParams { schema_v: 1, cell_id: "9q8yy".into(), epoch: 20667 };

    let mut ok = 0usize;
    for (id, r) in &cell.records {
        verify_self_contained(r, &params).unwrap_or_else(|e| {
            panic!("record {} FAILED verification: {e}", bs58::encode(id).into_string())
        });
        ok += 1;
        println!("  verified: \"{}\" (sig {} B)", r.headline, r.sig.len());
    }
    // The replay check, against real network bytes.
    let wrong = CellParams { schema_v: 1, cell_id: "dr5ru".into(), epoch: 20667 };
    for r in cell.records.values() {
        assert!(verify_self_contained(r, &wrong).is_err(), "must not verify in another cell");
    }
    println!("{ok} record(s) verified from network bytes; all reject replay into another cell");
}
