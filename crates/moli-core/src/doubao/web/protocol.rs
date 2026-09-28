//! Messages exchanged over the web ASR WebSocket.
//!
//! Upstream: raw 16 kHz mono s16le PCM in binary frames, then [`FINISH_FRAME`]
//! as a text frame when the user stops. Downstream: JSON text frames.

use serde::Deserialize;
use serde_json::Value;

use crate::asr::ServerMsg;

/// Text frame that asks the server to flush the final result and send `finish`.
pub const FINISH_FRAME: &str = r#"{"event":"finish"}"#;

/// Known non-zero `code` values (see `docs/doubao-wss-asr-spec.md` §0.6).
pub mod codes {
    /// Invalid session or tourist quota used up. Also sent as close reason
    /// [`super::TOURIST_LIMITED_CLOSE_REASON`] without any message first.
    pub const TOURIST_LIMITED: i64 = 710_022_013;
    /// From earlier notes; never seen live.
    pub const TIMEOUT: i64 = 709_599_053;
    /// From earlier notes; never seen live.
    pub const INVALID: i64 = 709_599_054;
}

/// Close reason the server uses instead of a [`codes::TOURIST_LIMITED`] message.
pub const TOURIST_LIMITED_CLOSE_REASON: &str = "2013";

/// Whether an error or close means the login is no longer accepted.
pub fn is_session_rejected(code: Option<i64>, close_reason: Option<&str>) -> bool {
    code == Some(codes::TOURIST_LIMITED) || close_reason == Some(TOURIST_LIMITED_CLOSE_REASON)
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
}

pub fn parse(text: &str) -> Result<ServerMsg, serde_json::Error> {
    let raw: Raw = serde_json::from_str(text)?;
    let code = raw.code.unwrap_or(0);
    if code != 0 {
        return Ok(ServerMsg::Error {
            code,
            message: raw.message.unwrap_or_default(),
        });
    }
    let event = raw.event.unwrap_or_default();
    Ok(match event.as_str() {
        "result" => ServerMsg::Result {
            text: raw
                .result
                .get("Text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "finish" => ServerMsg::Finish,
        _ => ServerMsg::Unknown { event },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_result() {
        let m = parse(r#"{"event":"result","result":{"Text":"你好"},"code":0,"message":""}"#);
        assert_eq!(
            m.unwrap(),
            ServerMsg::Result {
                text: "你好".into()
            }
        );
    }

    #[test]
    fn parses_result_without_text() {
        for s in [
            r#"{"event":"result","result":null,"code":0}"#,
            r#"{"event":"result","result":{},"code":0}"#,
            r#"{"event":"result"}"#,
        ] {
            assert_eq!(
                parse(s).unwrap(),
                ServerMsg::Result {
                    text: String::new()
                },
                "{s}"
            );
        }
    }

    #[test]
    fn parses_finish() {
        let m = parse(r#"{"event":"finish","result":null,"code":0,"message":""}"#);
        assert_eq!(m.unwrap(), ServerMsg::Finish);
    }

    #[test]
    fn nonzero_code_is_error_whatever_the_event() {
        let m = parse(r#"{"event":"result","result":null,"code":709599054,"message":"invalid"}"#);
        assert_eq!(
            m.unwrap(),
            ServerMsg::Error {
                code: codes::INVALID,
                message: "invalid".into()
            }
        );
    }

    #[test]
    fn session_rejection() {
        let m = parse(r#"{"code":710022013,"message":"tourist reach limited"}"#).unwrap();
        let ServerMsg::Error { code, .. } = m else {
            panic!("{m:?}")
        };
        assert!(is_session_rejected(Some(code), None));
        assert!(is_session_rejected(None, Some("2013")));
        assert!(!is_session_rejected(None, Some("1000-")));
        assert!(!is_session_rejected(Some(codes::INVALID), Some("")));
    }

    #[test]
    fn unknown_event_and_garbage() {
        assert_eq!(
            parse(r#"{"event":"ping","code":0}"#).unwrap(),
            ServerMsg::Unknown {
                event: "ping".into()
            }
        );
        assert!(parse("not json").is_err());
    }
}
