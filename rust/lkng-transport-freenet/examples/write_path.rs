//! Does seed-PUT + UPDATE on ONE session unblock multi-writer contracts?
//!
//! Usage: write_path <contract.wasm> <params.bin> <state.bin> <delta.bin>
use std::time::Duration;
use lkng_transport_freenet::FreenetClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (code, params, state, delta) = (
        std::fs::read(&a[0])?, std::fs::read(&a[1])?,
        std::fs::read(&a[2])?, std::fs::read(&a[3])?,
    );

    let mut c = FreenetClient::connect(
        lkng_transport_freenet::DEFAULT_NODE_URL, Duration::from_secs(120)).await?;
    println!("connected (one session for everything)");

    let key = FreenetClient::seed(&mut c, &code, &params, state).await?;
    println!("seed PUT ok: {key}");

    let before = c.get(&key, true).await?;
    println!("state before update: {} bytes", before.len());

    match c.update(&key, delta).await {
        Ok(()) => {
            let after = c.get(&key, false).await?;
            println!("UPDATE ACCEPTED — state now {} bytes (was {})", after.len(), before.len());
            if after.len() > before.len() {
                println!(">>> WRITE PATH WORKS: a second author's record merged in");
            } else {
                println!(">>> update accepted but state did not grow; inspect merge");
            }
        }
        Err(e) => println!("UPDATE FAILED: {e}"),
    }
    Ok(())
}
