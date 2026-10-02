//! The frames of the DashScope duplex recognition protocol.

use serde::Deserialize;
use serde_json::{Value, json};

/// The frame that opens the task; audio may follow once `task-started` is back.
pub fn run_task(task_id: &str, model: &str, sample_rate: u32) -> String {
    json!({
        "header": {"action": "run-task", "task_id": task_id, "streaming": "duplex"},
        "payload": {
            "task_group": "audio",
            "task": "asr",
            "function": "recognition",
            "model": model,
            "parameters": {"format": "pcm", "sample_rate": sample_rate},
            "input": {}
        }
    })
    .to_string()
}

/// The frame that ends the audio.
pub fn finish_task(task_id: &str) -> String {
    json!({
        "header": {"action": "finish-task", "task_id": task_id, "streaming": "duplex"},
        "payload": {"input": {}}
    })
    .to_string()
}

/// What the service sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Started,
    /// The sentence being recognized. `end` is set once it is final; the next
    /// event then starts a new sentence.
    Sentence {
        text: String,
        end: bool,
    },
    /// A keep-alive result without text.
    Heartbeat,
    Finished,
    Failed {
        code: String,
        message: String,
    },
    Unknown(String),
}

#[derive(Deserialize)]
struct Raw {
    header: Header,
    #[serde(default)]
    payload: Value,
}

#[derive(Deserialize)]
struct Header {
    event: String,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    error_message: Option<String>,
}

pub fn parse(text: &str) -> Result<Frame, serde_json::Error> {
    let raw: Raw = serde_json::from_str(text)?;
    Ok(match raw.header.event.as_str() {
        "task-started" => Frame::Started,
        "task-finished" => Frame::Finished,
        "task-failed" => Frame::Failed {
            code: raw.header.error_code.unwrap_or_default(),
            message: raw.header.error_message.unwrap_or_default(),
        },
        "result-generated" => {
            let sentence = &raw.payload["output"]["sentence"];
            if !sentence.is_object() || sentence["heartbeat"].as_bool() == Some(true) {
                Frame::Heartbeat
            } else {
                Frame::Sentence {
                    text: sentence["text"].as_str().unwrap_or_default().to_string(),
                    // Like the official SDK: a sentence is over once it has an end time.
                    end: !sentence["end_time"].is_null(),
                }
            }
        }
        other => Frame::Unknown(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_well_formed() {
        let v: Value = serde_json::from_str(&run_task("t1", "m", 16000)).unwrap();
        assert_eq!(v["header"]["action"], "run-task");
        assert_eq!(v["header"]["task_id"], "t1");
        assert_eq!(v["header"]["streaming"], "duplex");
        assert_eq!(v["payload"]["model"], "m");
        assert_eq!(v["payload"]["parameters"]["sample_rate"], 16000);
        assert_eq!(v["payload"]["function"], "recognition");
        let v: Value = serde_json::from_str(&finish_task("t1")).unwrap();
        assert_eq!(v["header"]["action"], "finish-task");
    }

    #[test]
    fn parses_events() {
        assert_eq!(
            parse(r#"{"header":{"event":"task-started"},"payload":{}}"#).unwrap(),
            Frame::Started
        );
        assert_eq!(
            parse(r#"{"header":{"event":"task-finished"},"payload":{"output":{}}}"#).unwrap(),
            Frame::Finished
        );
        assert_eq!(
            parse(
                r#"{"header":{"event":"result-generated"},"payload":{"output":{"sentence":
                {"begin_time":0,"end_time":null,"text":"你好"}}}}"#
            )
            .unwrap(),
            Frame::Sentence {
                text: "你好".into(),
                end: false
            }
        );
        assert_eq!(
            parse(
                r#"{"header":{"event":"result-generated"},"payload":{"output":{"sentence":
                {"begin_time":0,"end_time":1800,"text":"你好。"}}}}"#
            )
            .unwrap(),
            Frame::Sentence {
                text: "你好。".into(),
                end: true
            }
        );
        assert_eq!(
            parse(
                r#"{"header":{"event":"result-generated"},"payload":{"output":{"sentence":
                {"heartbeat":true}}}}"#
            )
            .unwrap(),
            Frame::Heartbeat
        );
        assert_eq!(
            parse(r#"{"header":{"event":"result-generated"},"payload":{"output":{}}}"#).unwrap(),
            Frame::Heartbeat
        );
        assert_eq!(
            parse(
                r#"{"header":{"event":"task-failed","error_code":"InvalidApiKey","error_message":"bad key"}}"#
            )
            .unwrap(),
            Frame::Failed {
                code: "InvalidApiKey".into(),
                message: "bad key".into()
            }
        );
        assert_eq!(
            parse(r#"{"header":{"event":"x"}}"#).unwrap(),
            Frame::Unknown("x".into())
        );
        assert!(parse("not json").is_err());
    }
}
