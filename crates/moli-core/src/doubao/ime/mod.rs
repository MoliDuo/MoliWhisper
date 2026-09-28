//! The ASR behind the Doubao input method: anonymous speech recognition,
//! plus a rewrite of spoken text as written text.
//!
//! Needs no login. The client presents a random device id; credentials
//! (an app key, then a token for it) are fetched at run time, kept in memory
//! only, and fetched again when stale or refused.
//!
//! Recognition runs over one WebSocket of protobuf frames ([`wire`]): open
//! it, `StartTask`, then any number of sessions (`StartSession`, audio,
//! `FinishSession` → `SessionFinished`) on that task. Opening takes seconds,
//! starting a session on an open task a few hundred ms, so a task is kept
//! open between sessions (see [`ImeClient::warm`]).
//!
//! The service splits speech into segments. Each result carries the
//! segment's text so far, its `index` and its `start_time`; the full text is
//! the segments in order. A pause starts a segment with the next index, but
//! after about 24 s of speech without one the service starts a segment with
//! the same index: only the start time tells it apart.
//!
//! The service routes sessions by device id, and some ids are never routed:
//! the session starts fine and fails as soon as audio arrives ("service
//! discovery failure"). The warm-up checks the id with a little silence;
//! a session that runs into it anyway is replayed under a new device id.
//!
//! See `docs/doubao-ime.md` for the protocol as observed.

mod api;
mod client;
mod connection;
mod credentials;
mod error;
mod pool;
mod session;
mod transcript;
pub mod wire;

pub use api::{IME_VERSION_CODE, Release};
pub use client::ImeClient;
pub use error::ApiError;
pub use session::{ImeSink, ImeStream};

/// The app id of the Doubao IME.
const AID: u32 = 685_343;
const DEVICE_PLATFORM: &str = "mac";
/// What the IME sends; its release is [`IME_VERSION_CODE`].
const USER_AGENT: &str = "DoubaoIme/1.0.1";

/// Where the service lives. The defaults are the production hosts; tests
/// point these at a mock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Remote settings, which carry the app key.
    pub settings: String,
    /// Trades the app key for a token.
    pub sami_config: String,
    /// The recognition WebSocket.
    pub asr_ws: String,
    /// Rewrites spoken text as written text.
    pub organize: String,
    /// Lists IME releases.
    pub version: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            settings: "https://is.snssdk.com/service/settings/v3/".into(),
            sami_config: "https://ime.oceancloudapi.com/api/v1/user/get_config".into(),
            asr_ws: "wss://frontier-audio-ime-ws.doubao.com/ocean/api/v1/ws".into(),
            organize: "https://ime.oceancloudapi.com/api/v2/ai/text_organization".into(),
            version: "https://ime.doubao.com/api/v1/version/list".into(),
        }
    }
}
