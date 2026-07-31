fn main() {
    let p = presence_cell::Params { schema_v: 1, cell_id: "9q8yy".into(), epoch: 20671 };
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&p, &mut buf).unwrap();
    std::fs::write("cell_params.bin", &buf).unwrap();
    println!("params: {} bytes", buf.len());
}
