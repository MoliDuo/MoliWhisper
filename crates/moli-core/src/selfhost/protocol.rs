//! The frames of the self-hosted ASR protocol (v1).

use serde::Deserialize;

use crate::asr::ServerMsg;

/// The text frame that ends the audio.
pub const FINISH_FRAME: &str = r#"{"type":"finish"}"#;

/// What the server sent, before it is mapped onto [`ServerMsg`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// The transcript so far.
    Partial(String),
    /// The final transcript; the server closes after it.
    Final(String),
    Error(String),
    Unknown(String),
}

#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    message: String,
}

pub fn parse(text: &str) -> Result<Frame, serde_json::Error> {
    let raw: Raw = serde_json::from_str(text)?;
    Ok(match raw.kind.as_str() {
        "partial" => Frame::Partial(raw.text),
        "final" => Frame::Final(raw.text),
        "error" => Frame::Error(raw.message),
        _ => Frame::Unknown(raw.kind),
    })
}

impl Frame {
    /// The messages the session sees for this frame: a final result is
    /// followed by `Finish`.
    pub fn into_msgs(self) -> Vec<ServerMsg> {
        match self {
            Frame::Partial(text) => vec![ServerMsg::Result { text }],
            Frame::Final(text) => vec![ServerMsg::Result { text }, ServerMsg::Finish],
            Frame::Error(message) => vec![ServerMsg::Error { code: -1, message }],
            Frame::Unknown(event) => vec![ServerMsg::Unknown { event }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frames() {
        assert_eq!(
            parse(r#"{"type":"partial","text":"你好"}"#).unwrap(),
            Frame::Partial("你好".into())
        );
        assert_eq!(
            parse(r#"{"type":"final","text":"你好。"}"#).unwrap(),
            Frame::Final("你好。".into())
        );
        assert_eq!(
            parse(r#"{"type":"error","message":"boom"}"#).unwrap(),
            Frame::Error("boom".into())
        );
        assert_eq!(
            parse(r#"{"type":"hello"}"#).unwrap(),
            Frame::Unknown("hello".into())
        );
        assert!(parse("not json").is_err());
    }

    #[test]
    fn final_is_followed_by_finish() {
        let msgs = Frame::Final("a".into()).into_msgs();
        assert_eq!(
            msgs,
            vec![ServerMsg::Result { text: "a".into() }, ServerMsg::Finish]
        );
    }
}
