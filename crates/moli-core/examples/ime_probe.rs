//! Probe for the live Doubao IME backend: connect time (cold and with cached
//! credentials), the final text, and the organize rewrite.
//!
//! ```text
//! RUST_LOG=moli_core=info cargo run -p moli-core --example ime_probe -- crates/moli-core/fixtures/zh_short.wav 3
//! ```
//!
//! Uses a new device id each run unless `IME_DEVICE_ID` is set. `IME_WARM=1`
//! keeps a connection open between sessions; `IME_GAP_MS=3000` plays the
//! recording twice with that much silence between, which the service
//! splits into segments. `IME_PACE_MS=0` sends faster than real time
//! (50 is real time). `IME_IDLE_S=70` waits that long before each
//! session, to see a kept-open connection last.

use std::time::{Duration, Instant};

use anyhow::Context;
use moli_core::asr::{AsrEvent, ServerMsg};
use moli_core::doubao::ime::{IME_VERSION_CODE, ImeClient};

const CHUNK_BYTES: usize = 1600;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let mut args = std::env::args().skip(1);
    let wav = args
        .next()
        .unwrap_or_else(|| "crates/moli-core/fixtures/zh_short.wav".into());
    let repeat: usize = args.next().map_or(Ok(3), |n| n.parse())?;

    // Several recordings, separated by commas, are played back to back.
    let mut pcm = Vec::new();
    for wav in wav.split(',') {
        let mut reader = hound::WavReader::open(wav).with_context(|| format!("opening {wav}"))?;
        for s in reader.samples::<i16>() {
            pcm.extend_from_slice(&s?.to_le_bytes());
        }
    }
    let pcm = match std::env::var("IME_GAP_MS") {
        Ok(ms) => {
            let gap = vec![0u8; ms.parse::<usize>()? * 32];
            [pcm.as_slice(), &gap, &pcm].concat()
        }
        Err(_) => pcm,
    };

    // IME_DEVICE_ID=3141592653589793 is one the service cannot route (as of 2026-09).
    let device_id = std::env::var("IME_DEVICE_ID").unwrap_or_else(|_| ImeClient::new_device_id());
    let client = ImeClient::new(device_id);
    if std::env::var_os("IME_WARM").is_some() {
        let t = Instant::now();
        client.set_keep_warm(true);
        tokio::spawn(client.clone().warm());
        while !client.is_warm() {
            anyhow::ensure!(t.elapsed() < Duration::from_secs(90), "no warm connection");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        println!("warm after {} ms", t.elapsed().as_millis());
    }
    match client.newer_version(Duration::from_secs(10)).await {
        Some(release) => println!("newer IME out: {} ({})", release.name, release.code),
        None => println!("no IME newer than {IME_VERSION_CODE}"),
    }
    // IME_PACE_MS=0 sends the recording as fast as the network allows.
    let pace = Duration::from_millis(std::env::var("IME_PACE_MS").map_or(Ok(50), |s| s.parse())?);
    let mut last = String::new();
    let idle = std::env::var("IME_IDLE_S").map_or(Ok(0), |s| s.parse())?;
    for i in 0..repeat {
        tokio::time::sleep(Duration::from_secs(idle)).await;
        let t = Instant::now();
        let (mut sink, mut stream, _) = client
            .connect(Duration::from_secs(10))
            .await
            .map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;
        let connected = t.elapsed();

        for chunk in pcm.chunks(CHUNK_BYTES) {
            sink.audio(chunk.to_vec())?;
            tokio::time::sleep(pace).await;
        }
        let stopped = Instant::now();
        sink.finish()?;
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                AsrEvent::Server(ServerMsg::Result { text: t }) => text = t,
                AsrEvent::Server(ServerMsg::Finish) => break,
                AsrEvent::Failed(why) => anyhow::bail!("session failed: {why}"),
                _ => {}
            }
        }
        println!(
            "#{i}: connect {} ms, final after {} ms: {text}",
            connected.as_millis(),
            stopped.elapsed().as_millis()
        );
        last = text;
    }

    let t = Instant::now();
    let organized = client.organize(&last, Duration::from_secs(5)).await;
    println!("organize {} ms: {organized:?}", t.elapsed().as_millis());
    let messy = "嗯那个就是说我们明天呃几点开会来着";
    let t = Instant::now();
    let organized = client.organize(messy, Duration::from_secs(5)).await;
    println!(
        "organize {} ms: {messy} -> {organized:?}",
        t.elapsed().as_millis()
    );
    Ok(())
}
