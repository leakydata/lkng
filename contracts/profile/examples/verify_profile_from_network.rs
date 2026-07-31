//! Verify a profile fetched from the network, and prove the two privacy
//! properties the design depends on.
use lkng_identity::{verify_profile, Identity};
use lkng_profile::{ProfileParams, ProfileState};

fn main() {
    let path = std::env::args().nth(1).expect("usage: <state.bin>");
    let bytes = std::fs::read(&path).expect("read");
    let state: ProfileState = ciborium::de::from_reader(&bytes[..]).expect("decode");

    let owner = Identity::from_seed([42u8; 32]);
    let params = owner.profile_params();

    verify_profile(&state, &params).expect("network bytes must verify");
    let b = state.body.as_ref().expect("body");
    println!("verified profile of \"{}\" (seq {})", b.display_name, b.sequence);

    // 1. Nobody else can mount this state at their own address.
    let other = Identity::from_seed([43u8; 32]);
    assert!(verify_profile(&state, &other.profile_params()).is_err());

    // 2. The epoch keys that sign public tiles appear nowhere in here, so
    //    holding a profile does not let you find that person's tiles.
    for epoch in [20666u64, 20667, 20668] {
        let ek = owner.for_epoch(epoch).verifying_key_bytes();
        assert!(
            !bytes.windows(ek.len()).any(|w| w == ek.as_slice()),
            "epoch key for {epoch} leaked into the profile"
        );
    }
    println!("no epoch key present; state is not transferable to another address");
}
