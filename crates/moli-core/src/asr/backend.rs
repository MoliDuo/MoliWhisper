//! Picks the recognition service for a session: the Doubao web ASR (needs a
//! login) or the Doubao IME (anonymous). Both look the same to the session.

use std::time::Duration;

use super::client::SendError;
use super::client::{self, AsrEvent, AsrSink, AsrStream, ConnectError, ConnectOptions, Handshake};
use crate::ime::{ImeClient, ImeSink, ImeStream};

#[derive(Clone)]
pub enum Backend {
    Web(ConnectOptions),
    Ime(ImeClient),
}

impl Backend {
    /// Connects within `timeout` (DNS, TLS, WebSocket and, for the IME,
    /// credentials and the session handshake).
    pub async fn connect(
        &self,
        timeout: Duration,
    ) -> Result<(Sink, Stream, Handshake), ConnectError> {
        match self {
            Backend::Web(opts) => {
                let mut opts = opts.clone();
                opts.timeout = timeout;
                let (sink, stream, handshake) = client::connect(&opts).await?;
                Ok((Sink::Web(sink), Stream::Web(stream), handshake))
            }
            Backend::Ime(ime) => {
                let (sink, stream, handshake) = ime.connect(timeout).await?;
                Ok((Sink::Ime(sink), Stream::Ime(stream), handshake))
            }
        }
    }
}

pub enum Sink {
    Web(AsrSink),
    Ime(ImeSink),
}

impl Sink {
    /// Sends a chunk of 16 kHz mono s16le PCM.
    pub async fn audio(&mut self, pcm: Vec<u8>) -> Result<(), SendError> {
        match self {
            Sink::Web(s) => s.audio(pcm).await,
            Sink::Ime(s) => s.audio(pcm),
        }
    }

    /// Tells the server the audio is over; it answers with the final result and `Finish`.
    pub async fn finish(&mut self) -> Result<(), SendError> {
        match self {
            Sink::Web(s) => s.finish().await,
            Sink::Ime(s) => s.finish(),
        }
    }

    pub async fn close(&mut self) -> Result<(), SendError> {
        match self {
            Sink::Web(s) => s.close().await,
            Sink::Ime(s) => s.close(),
        }
    }
}

pub enum Stream {
    Web(AsrStream),
    Ime(ImeStream),
}

impl Stream {
    pub async fn next(&mut self) -> Option<AsrEvent> {
        match self {
            Stream::Web(s) => s.next().await,
            Stream::Ime(s) => s.next().await,
        }
    }
}
