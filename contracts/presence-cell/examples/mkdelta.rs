use lkng_identity::Identity;
use lkng_presence::{CellParams, PresenceRecord};
fn main() {
    let id = Identity::from_seed([77u8; 32]); // a second user
    let params = CellParams { schema_v: 1, cell_id: "9q8yy".into(), epoch: 20673 };
    let mut r = PresenceRecord {
        pseudonym: [0; 32],
        headline: "second user, arriving by delta".into(),
        thumbnail: vec![3u8; 64],
        timestamp_ms: 1_785_525_000_000,
        age_band: 0,
        verifying_key: None,
        writer_cert: None,
        sig: vec![],
    };
    id.sign_presence(&mut r, &params).expect("sign");
    lkng_presence::verify::verify_self_contained(&r, &params).expect("verify");
    let delta = vec![r];
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&delta, &mut buf).unwrap();
    std::fs::write("delta.bin", &buf).unwrap();
    println!("delta: {} bytes (1 signed record)", buf.len());
}
