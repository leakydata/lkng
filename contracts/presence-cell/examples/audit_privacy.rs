//! Audit a network-fetched cell for the privacy property that matters:
//! does any published tile leak the owner's DURABLE identity?
use lkng_identity::Identity;
use lkng_presence::CellState;
fn main() {
    let path = std::env::args().nth(1).expect("usage: audit_privacy <state.bin>");
    let bytes = std::fs::read(&path).expect("read");
    let cell: CellState = ciborium::de::from_reader(&bytes[..]).expect("decode");

    // The identity that authored the demo tiles.
    let id = Identity::from_seed([42u8; 32]);
    let durable_vk = id.verifying_key_bytes();
    let durable_pseudonym = id.pseudonym();

    for r in cell.records.values() {
        let published = r.verifying_key.as_deref().expect("self-contained");
        assert_ne!(published, durable_vk.as_slice(), "LEAK: durable key published");
        assert_ne!(r.pseudonym, durable_pseudonym, "LEAK: durable pseudonym published");
        // The durable key must not appear anywhere in the raw bytes either.
        assert!(
            !bytes.windows(durable_vk.len()).any(|w| w == durable_vk.as_slice()),
            "LEAK: durable key found in raw published state"
        );
    }
    println!(
        "audit passed: {} tile(s), durable identity ({}) absent from published bytes",
        cell.records.len(),
        id.fingerprint()
    );
}
