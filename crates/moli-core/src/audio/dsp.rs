//! Turns captured audio into 16 kHz mono s16le chunks with a level for the overlay.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use crate::asr::protocol::SAMPLE_RATE;

/// Length of one chunk sent to the server. The server does not care; this
/// sets how often the overlay's level updates.
pub const CHUNK_MS: usize = 50;
pub const CHUNK_SAMPLES: usize = SAMPLE_RATE as usize * CHUNK_MS / 1000;
/// Frames per resampler call at the input rate.
const RESAMPLER_CHUNK: usize = 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// 16 kHz mono s16le.
    pub pcm: Vec<u8>,
    /// Loudness in 0..=1 for display (see [`level`]).
    pub level: f32,
}

/// Averages interleaved frames into mono.
pub fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32),
    );
}

pub fn to_s16le(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|&s| ((s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16).to_le_bytes())
        .collect()
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Maps RMS onto 0..=1 over a -60..0 dBFS range, which suits speech.
pub fn level(rms: f32) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    ((20.0 * rms.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Streaming converter from mono audio at the device rate to [`Chunk`]s.
pub struct Converter {
    resampler: Option<Fft<f32>>,
    input_rate: u32,
    pending_in: Vec<f32>,
    pending_out: Vec<f32>,
    out_buf: Vec<f32>,
    /// Output frames still to drop at the start (the resampler's delay).
    skip: usize,
    total_in: u64,
    total_out: u64,
}

impl Converter {
    pub fn new(input_rate: u32) -> Result<Self, String> {
        let resampler = if input_rate == SAMPLE_RATE {
            None
        } else {
            let r = Fft::<f32>::new(
                input_rate as usize,
                SAMPLE_RATE as usize,
                RESAMPLER_CHUNK,
                1,
                FixedSync::Input,
            )
            .map_err(|e| format!("resampler for {input_rate} Hz: {e}"))?;
            Some(r)
        };
        let skip = resampler.as_ref().map_or(0, |r| r.output_delay());
        let out_len = resampler.as_ref().map_or(0, |r| r.output_frames_max());
        Ok(Self {
            resampler,
            input_rate,
            pending_in: Vec::new(),
            pending_out: Vec::new(),
            out_buf: vec![0.0; out_len],
            skip,
            total_in: 0,
            total_out: 0,
        })
    }

    pub fn push(&mut self, mono: &[f32], out: &mut Vec<Chunk>) {
        self.total_in += mono.len() as u64;
        if self.resampler.is_none() {
            self.emit(mono);
        } else {
            self.pending_in.extend_from_slice(mono);
            self.resample_ready();
        }
        self.chunks(out);
    }

    /// Pushes out everything still buffered, the last chunk possibly short.
    pub fn flush(&mut self, out: &mut Vec<Chunk>) {
        if self.resampler.is_some() {
            let expected = self.total_in * SAMPLE_RATE as u64 / self.input_rate as u64;
            // Zero padding drives the tail (and the resampler delay) out.
            while self.total_out < expected {
                let need = self.resampler.as_ref().unwrap().input_frames_next();
                let pad = need.saturating_sub(self.pending_in.len());
                self.pending_in.extend(std::iter::repeat_n(0.0, pad));
                self.resample_ready();
            }
            let extra = (self.total_out - expected) as usize;
            self.pending_out
                .truncate(self.pending_out.len().saturating_sub(extra));
            self.total_out = expected;
            self.pending_in.clear();
        }
        self.chunks(out);
        if !self.pending_out.is_empty() {
            let rest = std::mem::take(&mut self.pending_out);
            out.push(chunk(&rest));
        }
    }

    fn resample_ready(&mut self) {
        // Taken out for the loop so `emit` can borrow `self`.
        let Some(mut r) = self.resampler.take() else {
            return;
        };
        loop {
            let need = r.input_frames_next();
            if self.pending_in.len() < need {
                break;
            }
            let input = InterleavedSlice::new(&self.pending_in[..need], 1, need)
                .expect("input buffer sized to frames");
            let frames = self.out_buf.len();
            let mut output = InterleavedSlice::new_mut(&mut self.out_buf, 1, frames)
                .expect("output buffer sized to frames");
            let (used, produced) = r
                .process_into_buffer(&input, &mut output, None)
                .expect("buffers sized from the resampler");
            self.pending_in.drain(..used);
            let produced = self.out_buf[..produced].to_vec();
            self.emit(&produced);
        }
        self.resampler = Some(r);
    }

    fn emit(&mut self, samples: &[f32]) {
        let skip = self.skip.min(samples.len());
        self.skip -= skip;
        let samples = &samples[skip..];
        self.total_out += samples.len() as u64;
        self.pending_out.extend_from_slice(samples);
    }

    fn chunks(&mut self, out: &mut Vec<Chunk>) {
        while self.pending_out.len() >= CHUNK_SAMPLES {
            out.push(chunk(&self.pending_out[..CHUNK_SAMPLES]));
            self.pending_out.drain(..CHUNK_SAMPLES);
        }
    }
}

fn chunk(samples: &[f32]) -> Chunk {
    Chunk {
        pcm: to_s16le(samples),
        level: level(rms(samples)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn run(rate: u32, input: &[f32], piece: usize) -> Vec<Chunk> {
        let mut c = Converter::new(rate).unwrap();
        let mut out = Vec::new();
        for p in input.chunks(piece) {
            c.push(p, &mut out);
        }
        c.flush(&mut out);
        out
    }

    #[test]
    fn output_length_matches_the_rate_ratio() {
        for rate in [16_000, 44_100, 48_000, 24_000] {
            let input = sine(rate, 440.0, 1.3);
            let out = run(rate, &input, 441);
            let samples: usize = out.iter().map(|c| c.pcm.len() / 2).sum();
            let expected = input.len() * 16_000 / rate as usize;
            assert_eq!(samples, expected, "rate {rate}");
            assert!(
                out[..out.len() - 1]
                    .iter()
                    .all(|c| c.pcm.len() == CHUNK_SAMPLES * 2)
            );
        }
    }

    #[test]
    fn keeps_speech_band_and_level() {
        let out = run(48_000, &sine(48_000, 1000.0, 1.0), 480);
        // Skip the edges where the filter ramps.
        let mid = &out[4..out.len() - 4];
        for c in mid {
            let s: Vec<f32> = c
                .pcm
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&b| i16::from_le_bytes(b) as f32 / i16::MAX as f32)
                .collect();
            let r = rms(&s);
            assert!((r - 0.5 / 2f32.sqrt()).abs() < 0.02, "rms {r}");
        }
    }

    #[test]
    fn s16_conversion_clamps() {
        assert_eq!(to_s16le(&[0.0, 1.0, -1.0, 2.0, -2.0]), {
            let mut v = Vec::new();
            for s in [0i16, 32767, -32767, 32767, -32767] {
                v.extend(s.to_le_bytes());
            }
            v
        });
    }

    #[test]
    fn downmix_and_level() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
        assert_eq!(level(0.0), 0.0);
        assert_eq!(level(1.0), 1.0);
        assert!((level(0.001) - 0.0).abs() < 1e-6);
        assert!((level(0.0316) - 0.5).abs() < 0.01);
    }
}
