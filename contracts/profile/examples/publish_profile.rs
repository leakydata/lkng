//! Build a signed profile + its parameters for publishing.
use lkng_identity::Identity;
use lkng_profile::ProfileBody;

fn main() {
    let id = Identity::from_seed([42u8; 32]); // demo seed; real ones come from the CSPRNG
    let params = id.profile_params();

    let body = ProfileBody {
        display_name: "sam".into(),
        bio: "building a dating app with nobody in the middle".into(),
        tags: vec!["rust".into(), "p2p".into(), "queer".into()],
        photos: vec![],
        thumbnail: vec![0u8; 96],
        sequence: 1,
    };
    let state = id.sign_profile(body).expect("sign");
    lkng_identity::verify_profile(&state, &params).expect("self-verify before publishing");

    let mut pbuf = Vec::new();
    ciborium::ser::into_writer(&params, &mut pbuf).unwrap();
    std::fs::write("profile_params.bin", &pbuf).unwrap();
    let mut sbuf = Vec::new();
    ciborium::ser::into_writer(&state, &mut sbuf).unwrap();
    std::fs::write("profile_state.bin", &sbuf).unwrap();

    println!(
        "handle {} | params {} B | state {} B",
        params.handle(), pbuf.len(), sbuf.len()
    );
}
