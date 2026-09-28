//! The ASR behind doubao.com, used with the cookies of a login.
//!
//! Each session is its own WebSocket to [`params::ENDPOINT`]: the query
//! carries the web client's identity ([`params`]), the handshake carries the
//! cookies ([`Credentials`]), then PCM goes up and JSON comes down
//! ([`protocol`]). An invalid login still completes the handshake and is
//! refused right after, which [`verify`] checks for.
//!
//! See `docs/doubao-wss-asr-spec.md` for the protocol as observed.

pub mod credentials;
pub mod params;
pub mod protocol;
mod transport;
mod verify;

pub use credentials::{Credentials, StoreCookie, cookies_for_url};
pub use transport::{ConnectOptions, DEFAULT_USER_AGENT, WebSink, WebStream, connect};
pub use verify::{Verdict, verify};
