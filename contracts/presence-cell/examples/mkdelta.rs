use lkng_presence::PresenceRecord;
fn main() {
    // Delta = Vec<PresenceRecord> (what update_state decodes)
    let delta = vec![PresenceRecord {
        pseudonym: *blake3::hash(b"lkng-second-user").as_bytes(),
        headline: "second tile — sent as a network delta".into(),
        thumbnail: vec![7u8; 64],
        timestamp_ms: 1_785_523_000_000,
        verifying_key: None,
        writer_cert: None,
        sig: vec![2u8; 64],
    }];
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&delta, &mut buf).unwrap();
    std::fs::write("delta.bin", &buf).unwrap();
    println!("delta: {} bytes", buf.len());
}
