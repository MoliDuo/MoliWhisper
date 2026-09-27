//! Debug builds only: `MOLI_TEST_AUDIO=file.wav` replaces the microphone with
//! a 16 kHz mono s16 WAV played at real time, then silence until stopped.
//! Lets the whole hotkey → ASR → paste path run without speaking.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use moli_core::audio::dsp::{self, CHUNK_MS, CHUNK_SAMPLES};
use moli_core::audio::{AudioEvent, AudioInput, Chunk};
use tokio::sync::mpsc;

pub fn from_env() -> Option<AudioInput> {
    let path = std::env::var_os("MOLI_TEST_AUDIO")?;
    let (tx, rx) = mpsc::unbounded_channel();
    let samples = match read_wav(std::path::Path::new(&path)) {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(AudioEvent::Failed(e));
            return Some(AudioInput::new(rx, || {}));
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    std::thread::spawn(move || {
        let _ = tx.send(AudioEvent::Started(format!("test audio {path:?}")));
        let start = Instant::now();
        let silence = [0f32; CHUNK_SAMPLES];
        for i in 0.. {
            if stop_thread.load(Ordering::Relaxed) {
                break;
            }
            let from = i * CHUNK_SAMPLES;
            let piece = samples
                .get(from..(from + CHUNK_SAMPLES).min(samples.len()))
                .filter(|p| !p.is_empty())
                .unwrap_or(&silence);
            let chunk = Chunk {
                pcm: dsp::to_s16le(piece),
                level: dsp::level(dsp::rms(piece)),
            };
            if tx.send(AudioEvent::Chunk(chunk)).is_err() {
                break;
            }
            let due = start + Duration::from_millis(((i + 1) * CHUNK_MS) as u64);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
    });
    Some(AudioInput::new(rx, move || {
        stop.store(true, Ordering::Relaxed)
    }))
}

fn read_wav(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let data = std::fs::read(path).map_err(|e| format!("{path:?}: {e}"))?;
    // Walk the RIFF chunks to "data"; assume 16 kHz mono s16le.
    let mut at = 12;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let len = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            let end = (at + 8 + len).min(data.len());
            return Ok(data[at + 8..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&b| i16::from_le_bytes(b) as f32 / i16::MAX as f32)
                .collect());
        }
        at += 8 + len + (len & 1);
    }
    Err(format!("{path:?}: no data chunk"))
}
