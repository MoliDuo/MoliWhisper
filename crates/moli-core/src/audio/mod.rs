//! Microphone input, converted to what the ASR takes.

pub mod capture;
pub mod dsp;

use tokio::sync::mpsc;

pub use dsp::Chunk;

#[derive(Debug)]
pub enum AudioEvent {
    /// The device is open; carries a description for the log.
    Started(String),
    Chunk(Chunk),
    /// The device could not be opened or stopped working. No more events follow.
    Failed(String),
}

/// A running capture. Events end (the channel closes) after [`AudioInput::stop`]
/// once the buffered tail has been delivered. Dropping it stops the capture too.
pub struct AudioInput {
    pub events: mpsc::UnboundedReceiver<AudioEvent>,
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl AudioInput {
    pub fn new(
        events: mpsc::UnboundedReceiver<AudioEvent>,
        stop: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            events,
            stop: Some(Box::new(stop)),
        }
    }

    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

impl Drop for AudioInput {
    fn drop(&mut self) {
        self.stop();
    }
}
