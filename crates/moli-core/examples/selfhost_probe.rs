//! Streams a few seconds of silence to a self-hosted ASR server and prints
//! what comes back: `cargo run -p moli-core --example selfhost_probe -- ws://host:8765/v1/stream [token]`

use std::time::Duration;

use moli_core::asr::{AsrEvent, Backend};
use moli_core::selfhost::{self, ConnectOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: selfhost_probe <ws-url> [token]"))?;
    let token = args.next().unwrap_or_default();

    println!("healthz: {:?}", selfhost::health(&url, &token).await);
    let opts =
        ConnectOptions::new(&url, &token).ok_or_else(|| anyhow::anyhow!("not a ws:// URL"))?;
    let (mut sink, mut stream, handshake) = Backend::SelfHosted(opts)
        .connect(Duration::from_secs(5))
        .await?;
    println!("connected in {:?}", handshake.elapsed);

    for _ in 0..60 {
        sink.audio(vec![0; 1600]).await?; // 50 ms of silence
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    sink.finish().await?;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(10), stream.next()).await? {
        println!("{event:?}");
        if matches!(event, AsrEvent::Closed { .. } | AsrEvent::Failed(_)) {
            break;
        }
    }
    Ok(())
}
