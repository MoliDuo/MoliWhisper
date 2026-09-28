//! Client for the Doubao web ASR WebSocket.
//!
//! The protocol is an external interface we have to follow; see
//! `docs/doubao-wss-asr-spec.md` for what is known about it.

pub mod backend;
pub mod client;
pub mod params;
pub mod protocol;
pub mod verify;

pub use backend::Backend;
pub use client::{AsrEvent, AsrSink, AsrStream, ConnectError, ConnectOptions, connect};
pub use protocol::ServerMsg;
pub use verify::{Verdict, verify};
