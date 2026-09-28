//! Clients for Doubao's speech recognition.
//!
//! Two services recognize speech, and the app can use either:
//!
//! - [`web`]: the ASR behind doubao.com. Needs the cookies of a login; raw PCM
//!   up and JSON down over one WebSocket per session.
//! - [`ime`]: the ASR behind the Doubao input method. Anonymous, with
//!   credentials fetched at run time; protobuf frames over a WebSocket that
//!   is kept open between sessions. Also rewrites spoken text as written text.
//!
//! Both are private interfaces, followed as observed: see
//! `docs/doubao-wss-asr-spec.md` and `docs/doubao-ime.md`.

pub mod ime;
pub(crate) mod net;
pub mod web;
