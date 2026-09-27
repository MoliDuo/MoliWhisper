//! Microphone capture with cpal.
//!
//! The stream lives on its own thread (a cpal `Stream` is not `Send` on every
//! backend). The device callback only downmixes into a lock-free ring; the
//! same thread drains the ring every few milliseconds, converts and sends
//! chunks. Opening the device happens there too, so starting never blocks the
//! caller and overlaps with connecting to the server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use tokio::sync::mpsc;

use super::dsp::{Converter, downmix};
use super::{AudioEvent, AudioInput};

const DRAIN_EVERY: Duration = Duration::from_millis(10);
/// Seconds of audio the ring holds if draining stalls.
const RING_SECONDS: usize = 2;

/// Starts capturing from the default input device.
pub fn start() -> AudioInput {
    let (tx, rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("audio-capture".into())
        .spawn(move || {
            if let Err(e) = run(&tx, &stop_thread) {
                let _ = tx.send(AudioEvent::Failed(e));
            }
        });
    if let Err(e) = spawned {
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(AudioEvent::Failed(format!(
            "could not start audio thread: {e}"
        )));
        return AudioInput::new(rx, || {});
    }
    AudioInput::new(rx, move || stop.store(true, Ordering::Relaxed))
}

fn run(tx: &mpsc::UnboundedSender<AudioEvent>, stop: &AtomicBool) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no input device".to_string())?;
    let name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_else(|_| "unknown device".into());
    let supported = device
        .default_input_config()
        .map_err(|e| format!("{name}: {e}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let channels = config.channels as usize;
    let rate = config.sample_rate;

    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(rate as usize * RING_SECONDS);
    let failure: Arc<Mutex<Option<String>>> = Arc::default();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, producer, &failure),
        SampleFormat::I16 => build::<i16>(&device, &config, producer, &failure),
        SampleFormat::I32 => build::<i32>(&device, &config, producer, &failure),
        SampleFormat::U16 => build::<u16>(&device, &config, producer, &failure),
        SampleFormat::I8 => build::<i8>(&device, &config, producer, &failure),
        other => return Err(format!("{name}: unsupported sample format {other}")),
    }
    .map_err(|e| format!("{name}: {e}"))?;
    stream.play().map_err(|e| format!("{name}: {e}"))?;

    let mut converter = Converter::new(rate)?;
    let _ = tx.send(AudioEvent::Started(format!(
        "{name} ({rate} Hz, {channels} ch, {format})"
    )));

    let mut mono = Vec::new();
    let mut chunks = Vec::new();
    loop {
        std::thread::sleep(DRAIN_EVERY);
        let stopping = stop.load(Ordering::Relaxed);
        if stopping {
            // Stop the device first so nothing arrives after the final drain.
            drop_stream(stream);
            drain(&mut consumer, &mut converter, &mut mono, &mut chunks);
            converter.flush(&mut chunks);
            send(tx, &mut chunks);
            return Ok(());
        }
        drain(&mut consumer, &mut converter, &mut mono, &mut chunks);
        if !send(tx, &mut chunks) {
            return Ok(()); // nobody listens any more
        }
        if let Some(e) = failure.lock().unwrap().take() {
            return Err(format!("{name}: {e}"));
        }
    }
}

fn drain(
    consumer: &mut rtrb::Consumer<f32>,
    converter: &mut Converter,
    mono: &mut Vec<f32>,
    chunks: &mut Vec<super::Chunk>,
) {
    mono.clear();
    if let Ok(read) = consumer.read_chunk(consumer.slots()) {
        let (a, b) = read.as_slices();
        mono.extend_from_slice(a);
        mono.extend_from_slice(b);
        read.commit_all();
    }
    converter.push(mono, chunks);
}

fn drop_stream(stream: Stream) {
    let _ = stream.pause();
    drop(stream);
}

fn send(tx: &mpsc::UnboundedSender<AudioEvent>, chunks: &mut Vec<super::Chunk>) -> bool {
    for c in chunks.drain(..) {
        if tx.send(AudioEvent::Chunk(c)).is_err() {
            return false;
        }
    }
    true
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut producer: rtrb::Producer<f32>,
    failure: &Arc<Mutex<Option<String>>>,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let failure = failure.clone();
    let mut scratch: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    device.build_input_stream::<T, _, _>(
        *config,
        move |data: &[T], _| {
            // Allocates only until the buffers reach the device's block size.
            scratch.clear();
            scratch.extend(data.iter().map(|&s| s.to_sample::<f32>()));
            mono.clear();
            downmix(&scratch, channels, &mut mono);
            let n = mono.len().min(producer.slots());
            if let Ok(mut w) = producer.write_chunk_uninit(n) {
                let (a, b) = w.as_mut_slices();
                for (slot, &s) in a.iter_mut().chain(b.iter_mut()).zip(&mono) {
                    slot.write(s);
                }
                // SAFETY: all n slots were written just above.
                unsafe { w.commit_all() };
            }
        },
        move |e: cpal::Error| {
            use cpal::ErrorKind::*;
            match e.kind() {
                // Recoverable or informational; the stream keeps running.
                Xrun | RealtimeDenied | DeviceChanged => tracing::warn!("audio: {e}"),
                _ => *failure.lock().unwrap() = Some(e.to_string()),
            }
        },
        None,
    )
}
