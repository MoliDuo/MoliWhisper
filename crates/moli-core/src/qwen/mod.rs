//! Qwen speech recognition on Alibaba Cloud (DashScope / Model Studio).
//!
//! One WebSocket per session. After the handshake we send `run-task` and
//! wait for `task-started`; audio goes up as binary PCM frames and
//! `finish-task` ends it. The service answers with `result-generated`
//! events for the sentence being spoken and `task-finished` at the end.

pub mod protocol;
mod transport;

pub use transport::{ConnectOptions, QwenSink, QwenStream, check, connect};

pub const DEFAULT_URL: &str = "wss://maas.qwencloudapi.com/api-ws/v1/inference";
pub const DEFAULT_MODEL: &str = "qwen-audio-3.0-asr-flash-streaming";
