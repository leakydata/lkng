//! Fetch an inbox from the network and try to read it — as the owner,
//! and as everyone else.
use lkng_identity::Identity;
use lkng_inbox::InboxState;
fn main() {
    let path = std::env::args().nth(1).expect("usage: read_inbox <state.bin>");
    let bytes = std::fs::read(&path).expect("read");
    let state: InboxState = ciborium::de::from_reader(&bytes[..]).expect("decode");

    let bob = Identity::from_seed([0xB0; 32]);
    lkng_inbox::verify::verify_state(&state, &bob.inbox_params()).expect("network bytes verify");

    println!("{} pending message(s) in bob's inbox", state.pending().len());
    for env in state.pending() {
        let text = bob.open_message(env).expect("bob can open his own mail");
        println!("  from epoch-key {}…: \"{}\"",
            &bs58::encode(&env.sender_epoch_vk[..6]).into_string(),
            String::from_utf8_lossy(&text));

        // Everyone else, including the sender's other identity, is locked out.
        for (name, seed) in [("eve", 0xE5u8), ("mallory", 0x77)] {
            let stranger = Identity::from_seed([seed; 32]);
            assert!(stranger.open_message(env).is_err(), "{name} must not read this");
        }
        // And the plaintext never appears in the bytes that crossed the wire.
        assert!(!bytes.windows(text.len()).any(|w| w == text.as_slice()),
            "plaintext leaked into published state");
    }
    println!("no stranger can decrypt; plaintext absent from the published bytes");
}
