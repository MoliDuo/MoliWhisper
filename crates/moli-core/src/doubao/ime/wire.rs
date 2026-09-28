//! The frames on the recognition WebSocket, as protobuf. The IME's schema:
//!
//! ```proto
//! syntax = "proto3";
//! package mammon_internal;
//!
//! message WebSocketRequest {
//!   string token = 1;       // the token, on StartSession and FinishSession
//!   string appkey = 2;
//!   string namespace = 3;   // "ASR"
//!   string version = 4;     // not checked
//!   string event = 5;
//!   string payload = 6;     // JSON
//!   bytes audio_data = 7;   // 16 kHz mono s16le PCM
//!   string session_id = 8;
//! }
//!
//! message WebSocketResponse {
//!   string task_id = 1;     // the same for the whole connection
//!   string message_id = 2;
//!   string namespace = 3;
//!   string event = 4;       // empty on results
//!   int64 status_code = 5;  // see `status`
//!   string status_text = 6;
//!   string payload = 7;     // JSON; on results, {"results": [...]}
//!   string session_id = 8;
//!   int64 flag = 9;
//!   string logid = 11;
//! }
//! ```

use prost::Message as _;

/// A frame to the service.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Request {
    #[prost(string, tag = "1")]
    pub token: String,
    #[prost(string, tag = "2")]
    pub appkey: String,
    #[prost(string, tag = "3")]
    pub namespace: String,
    #[prost(string, tag = "4")]
    pub version: String,
    #[prost(string, tag = "5")]
    pub event: String,
    #[prost(string, tag = "6")]
    pub payload: String,
    #[prost(bytes = "vec", tag = "7")]
    pub audio_data: Vec<u8>,
    #[prost(string, tag = "8")]
    pub session_id: String,
}

/// A frame from the service.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Response {
    #[prost(string, tag = "1")]
    pub task_id: String,
    #[prost(string, tag = "2")]
    pub message_id: String,
    #[prost(string, tag = "3")]
    pub namespace: String,
    #[prost(string, tag = "4")]
    pub event: String,
    #[prost(int64, tag = "5")]
    pub status_code: i64,
    #[prost(string, tag = "6")]
    pub status_text: String,
    #[prost(string, tag = "7")]
    pub payload: String,
    #[prost(string, tag = "8")]
    pub session_id: String,
    #[prost(int64, tag = "9")]
    pub flag: i64,
    #[prost(string, tag = "11")]
    pub logid: String,
}

/// Event names.
pub mod event {
    // From the client.
    pub const START_TASK: &str = "StartTask";
    pub const START_SESSION: &str = "StartSession";
    /// Carries audio.
    pub const TASK_REQUEST: &str = "TaskRequest";
    pub const FINISH_SESSION: &str = "FinishSession";
    pub const FINISH_TASK: &str = "FinishTask";
    pub const PING: &str = "Ping";

    // From the service.
    pub const TASK_STARTED: &str = "TaskStarted";
    pub const SESSION_STARTED: &str = "SessionStarted";
    pub const PONG: &str = "Pong";
    pub const TASK_FAILED: &str = "TaskFailed";
    pub const SESSION_FAILED: &str = "SessionFailed";
    pub const SESSION_FINISHED: &str = "SessionFinished";
    pub const TASK_FINISHED: &str = "TaskFinished";
    /// Results come without an event name.
    pub const RESULT: &str = "";
}

/// `status_code` values.
pub mod status {
    pub const OK: i64 = 20_000_000;
    /// The service cannot route sessions for this device id.
    pub const UNROUTABLE: i64 = 50_700_000;
}

impl Request {
    pub fn new(event: &str) -> Self {
        Self {
            event: event.to_owned(),
            ..Self::default()
        }
    }

    pub fn appkey(mut self, appkey: &str) -> Self {
        self.appkey = appkey.to_owned();
        self
    }

    pub fn token(mut self, token: &str) -> Self {
        self.token = token.to_owned();
        self
    }

    pub fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = namespace.to_owned();
        self
    }

    pub fn payload(mut self, payload: &str) -> Self {
        self.payload = payload.to_owned();
        self
    }

    pub fn audio(mut self, pcm: &[u8]) -> Self {
        self.audio_data = pcm.to_vec();
        self
    }

    /// Fields left empty are not written, as proto3 does.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(data)
    }
}

impl Response {
    pub fn to_bytes(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(data)
    }

    /// Whether the service reports the task or the session failed.
    pub fn is_failure(&self) -> bool {
        self.event == event::TASK_FAILED || self.event == event::SESSION_FAILED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fields_are_not_written() {
        // Field 5 (event), length 4.
        assert_eq!(Request::new(event::PING).to_bytes(), b"\x2a\x04Ping");
    }

    #[test]
    fn a_request_survives_the_round_trip() {
        let wire = Request::new(event::START_SESSION)
            .appkey("AK")
            .token("T")
            .namespace("ASR")
            .payload("{}")
            .audio(&[1, 2])
            .to_bytes();
        let back = Request::from_bytes(&wire).unwrap();
        assert_eq!(back.appkey, "AK");
        assert_eq!(back.audio_data, vec![1, 2]);
    }

    #[test]
    fn logid_is_tag_11() {
        let r = Response {
            logid: "L".into(),
            ..Response::default()
        };
        assert_eq!(r.to_bytes(), b"\x5a\x01L");
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Byte for byte what code generated by protoc writes and reads.
    #[test]
    fn matches_the_reference_encoding() {
        let req = Request::new(event::START_SESSION)
            .appkey("AK")
            .token("T")
            .namespace("ASR")
            .payload("{}")
            .audio(&[1, 2]);
        assert_eq!(
            req.to_bytes(),
            unhex("0a01541202414b1a034153522a0c537461727453657373696f6e32027b7d3a020102")
        );

        let resp = Response::from_bytes(&unhex(
            "0a0174220d53657373696f6e4661696c6564288cb4891332036261643a027b7d5a014c",
        ))
        .unwrap();
        assert_eq!(resp.task_id, "t");
        assert_eq!(resp.event, event::SESSION_FAILED);
        assert_eq!(resp.status_code, 40_000_012);
        assert_eq!(resp.status_text, "bad");
        assert_eq!(resp.logid, "L");
        assert!(resp.is_failure());
    }
}
