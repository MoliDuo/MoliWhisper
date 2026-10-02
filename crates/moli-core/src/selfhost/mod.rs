//! A speech recognition server of our own (`server/` in the repository),
//! running Qwen3-ASR.
//!
//! One WebSocket per session, like the Doubao web ASR: raw PCM up as binary
//! frames, then `{"type":"finish"}`; `partial` and `final` JSON events down,
//! each with the full transcript so far.

pub mod protocol;
mod transport;

pub use transport::{ConnectOptions, SelfHostSink, SelfHostStream, connect, health};
