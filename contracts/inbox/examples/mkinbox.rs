use lkng_identity::Identity;
use lkng_inbox::InboxState;
fn main() {
    // Bob's inbox; alice sends him a first message.
    let bob = Identity::from_seed([0xB0; 32]);
    let alice = Identity::from_seed([0xA1; 32]);
    let params = bob.inbox_params();

    let env = alice.seal_message(
        &bob.encryption_public_key(), &bob.verifying_key_bytes(),
        20674, b"saw your tile - fancy a coffee?", 1_785_530_000_000,
    ).expect("seal");
    let mut state = InboxState::default();
    state.insert(env);
    lkng_inbox::verify::verify_state(&state, &params).expect("verify before publish");

    let mut p = Vec::new(); ciborium::ser::into_writer(&params, &mut p).unwrap();
    std::fs::write("inbox_params.bin", &p).unwrap();
    let mut s = Vec::new(); ciborium::ser::into_writer(&state, &mut s).unwrap();
    std::fs::write("inbox_state.bin", &s).unwrap();
    println!("params {} B | state {} B | 1 sealed envelope", p.len(), s.len());
}
