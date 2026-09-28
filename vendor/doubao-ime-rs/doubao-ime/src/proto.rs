//! ASR 帧协议：`mammon_internal.WebSocketRequest / WebSocketResponse`。
//!
//! 与 `proto/mammon_websocket.proto` 一一对应，用 prost 派生实现，无需 protoc。
//!
//! ```text
//! 请求: token=1  appkey=2  namespace=3  version=4  event=5
//!       payload=6  audio_data=7  session_id=8
//! 响应: task_id=1 message_id=2 namespace=3 event=4 status_code=5
//!       status_text=6 payload=7 session_id=8 flag=9 logid=11
//! ```

use prost::Message;

/// 客户端 → 服务端帧。
#[derive(Clone, PartialEq, Message)]
pub struct WebSocketRequest {
    /// SAMI JWT。
    #[prost(string, tag = "1")]
    pub token: String,
    /// asr_config.app_key。
    #[prost(string, tag = "2")]
    pub appkey: String,
    /// 会话命名空间，识别固定为 `"ASR"`。
    #[prost(string, tag = "3")]
    pub namespace: String,
    /// 协议版本（服务端当前不校验）。
    #[prost(string, tag = "4")]
    pub version: String,
    /// 事件名，见 [`event`]。
    #[prost(string, tag = "5")]
    pub event: String,
    /// JSON 载荷。
    #[prost(string, tag = "6")]
    pub payload: String,
    /// 16kHz 单声道 s16le PCM。
    #[prost(bytes = "vec", tag = "7")]
    pub audio_data: Vec<u8>,
    /// 会话 ID（可选）。
    #[prost(string, tag = "8")]
    pub session_id: String,
}

/// 服务端 → 客户端帧。
#[derive(Clone, PartialEq, Message)]
pub struct WebSocketResponse {
    /// 任务 ID，一次连接内不变。
    #[prost(string, tag = "1")]
    pub task_id: String,
    /// 消息 ID，每帧唯一。
    #[prost(string, tag = "2")]
    pub message_id: String,
    /// 回显命名空间。
    #[prost(string, tag = "3")]
    pub namespace: String,
    /// 事件名；**空字符串表示识别结果帧**。
    #[prost(string, tag = "4")]
    pub event: String,
    /// 状态码，20000000 为成功。
    #[prost(int64, tag = "5")]
    pub status_code: i64,
    /// 状态描述。
    #[prost(string, tag = "6")]
    pub status_text: String,
    /// 识别结果 JSON（event 为空时）。
    #[prost(string, tag = "7")]
    pub payload: String,
    /// 会话 ID（保留）。
    #[prost(string, tag = "8")]
    pub session_id: String,
    /// 当前恒为 0。
    #[prost(int64, tag = "9")]
    pub flag: i64,
    /// 链路追踪 ID。
    #[prost(string, tag = "11")]
    pub logid: String,
}

/// 事件名常量。
pub mod event {
    /// 客户端：开始任务。
    pub const START_TASK: &str = "StartTask";
    /// 客户端：开始会话。
    pub const START_SESSION: &str = "StartSession";
    /// 客户端：音频帧。
    pub const TASK_REQUEST: &str = "TaskRequest";
    /// 客户端：结束会话。
    pub const FINISH_SESSION: &str = "FinishSession";
    /// 客户端：结束任务。
    pub const FINISH_TASK: &str = "FinishTask";
    /// 客户端：心跳。
    pub const PING: &str = "Ping";
    /// 服务端：任务已开始。
    pub const TASK_STARTED: &str = "TaskStarted";
    /// 服务端：会话已开始。
    pub const SESSION_STARTED: &str = "SessionStarted";
    /// 服务端：心跳回复。
    pub const PONG: &str = "Pong";
    /// 服务端：任务失败。
    pub const TASK_FAILED: &str = "TaskFailed";
    /// 服务端：会话失败。
    pub const SESSION_FAILED: &str = "SessionFailed";
    /// 服务端：会话已结束。
    pub const SESSION_FINISHED: &str = "SessionFinished";
    /// 服务端：任务已结束。
    pub const TASK_FINISHED: &str = "TaskFinished";
    /// 服务端：识别结果帧（事件名为空）。
    pub const RESULT: &str = "";
}

impl WebSocketRequest {
    /// 以事件名开始构造请求帧。
    pub fn new(event: &str) -> Self {
        Self {
            event: event.to_owned(),
            ..Default::default()
        }
    }

    /// 设置 appkey。
    pub fn appkey(mut self, v: &str) -> Self {
        self.appkey = v.to_owned();
        self
    }

    /// 设置 token。
    pub fn token(mut self, v: &str) -> Self {
        self.token = v.to_owned();
        self
    }

    /// 设置命名空间。
    pub fn namespace(mut self, v: &str) -> Self {
        self.namespace = v.to_owned();
        self
    }

    /// 设置 JSON 载荷。
    pub fn payload(mut self, v: &str) -> Self {
        self.payload = v.to_owned();
        self
    }

    /// 设置音频数据。
    pub fn audio(mut self, pcm: &[u8]) -> Self {
        self.audio_data = pcm.to_vec();
        self
    }

    /// 序列化为线上字节（proto3：默认值字段不会出现在输出中）。
    pub fn to_bytes(&self) -> Vec<u8> {
        self.encode_to_vec()
    }
}

impl WebSocketResponse {
    /// 从线上字节解析。
    pub fn from_bytes(data: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(data)
    }

    /// 是否是失败事件。
    pub fn is_failure(&self) -> bool {
        self.event == event::TASK_FAILED || self.event == event::SESSION_FAILED
    }

    /// 结果帧中的文本（累积全文），见 [`extract_text`]。
    pub fn text(&self) -> Option<String> {
        extract_text(&self.payload)
    }
}

/// 从结果帧 payload JSON 中取出文本。
///
/// 服务端的 `text` 是**累积全文**而非增量，因此取最后一个非空 `text`。
/// payload 为空、不是合法 JSON 或不含文本时返回 `None`。
pub fn extract_text(payload: &str) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    v.get("results")?
        .as_array()?
        .iter()
        .filter_map(|it| it.get("text")?.as_str())
        .filter(|t| !t.is_empty())
        .last()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fields_are_not_serialized() {
        // field 5 (event) = "Ping"：tag 0x2a, len 4
        assert_eq!(WebSocketRequest::new("Ping").to_bytes(), b"\x2a\x04Ping");
    }

    #[test]
    fn roundtrip() {
        let wire = WebSocketRequest::new(event::START_SESSION)
            .appkey("AK")
            .token("T")
            .namespace("ASR")
            .payload("{}")
            .audio(&[1, 2])
            .to_bytes();
        let back = WebSocketRequest::decode(wire.as_slice()).unwrap();
        assert_eq!(back.appkey, "AK");
        assert_eq!(back.audio_data, vec![1, 2]);
    }

    #[test]
    fn response_logid_is_tag_11() {
        let r = WebSocketResponse {
            logid: "L".into(),
            flag: 0,
            ..Default::default()
        };
        assert_eq!(r.encode_to_vec(), b"\x5a\x01L");
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// 与 Python protobuf 运行时（protoc 生成代码）产出的字节逐字节比对。
    #[test]
    fn wire_compatible_with_reference_implementation() {
        let req = WebSocketRequest::new(event::START_SESSION)
            .appkey("AK")
            .token("T")
            .namespace("ASR")
            .payload("{}")
            .audio(&[1, 2]);
        assert_eq!(
            req.to_bytes(),
            unhex("0a01541202414b1a034153522a0c537461727453657373696f6e32027b7d3a020102")
        );

        let resp = WebSocketResponse::from_bytes(&unhex(
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

    #[test]
    fn text_extraction() {
        assert_eq!(
            extract_text(r#"{"results":[{"text":"a"},{"text":"ab"}]}"#).as_deref(),
            Some("ab")
        );
        assert_eq!(extract_text(r#"{"results":[]}"#), None);
        assert_eq!(extract_text("oops"), None);
        assert_eq!(extract_text(""), None);
    }
}
