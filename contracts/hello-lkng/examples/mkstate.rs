fn main() {
    let mut s = hello_lkng::HelloState::default();
    s.entries.insert("lkng-genesis-2026-07-31".to_string());
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&s, &mut buf).unwrap();
    std::fs::write("initial_state.bin", &buf).unwrap();
    println!("wrote {} bytes", buf.len());
}
