//! 错误类型。

use std::fmt;

/// 本库统一的 `Result` 别名。
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// 本库所有操作可能返回的错误。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// HTTP 层失败（连接、TLS、状态码、响应体读取）。
    #[error("HTTP 请求失败: {0}")]
    Http(#[from] reqwest::Error),

    /// WebSocket 层失败。
    #[error("WebSocket 错误: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),

    /// 服务端帧无法按 protobuf 解码。
    #[error("协议帧解码失败: {0}")]
    Decode(#[from] prost::DecodeError),

    /// JSON 解析失败。
    #[error("JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),

    /// 文件读写失败。
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 获取 app_key / sami_token / keyhub 票据失败。
    #[error("鉴权失败: {0}")]
    Auth(String),

    /// 识别会话失败（服务端返回 TaskFailed / SessionFailed，或事件流不符合预期）。
    #[error("识别失败: {0}")]
    Asr(String),

    /// 服务端以业务错误码拒绝请求。
    #[error("服务端错误 {code}: {message}")]
    Api {
        /// 服务端错误码。
        code: i64,
        /// 服务端错误描述。
        message: String,
    },

    /// 配置缺失或不合法（TNC 配置、信封配置、客户端配置）。
    #[error("配置错误: {0}")]
    Config(String),

    /// 音频不是 16kHz 单声道 16bit PCM。
    #[error("音频格式错误: {0}")]
    InvalidAudio(String),

    /// 加解密或密钥处理失败。
    #[error("加密错误: {0}")]
    Crypto(String),

    /// 操作超时。
    #[error("超时: {0}")]
    Timeout(&'static str),
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::WebSocket(Box::new(e))
    }
}

/// 错误分类，便于调用方（以及 FFI）按类别处理，而不必匹配完整枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
#[non_exhaustive]
pub enum ErrorKind {
    /// 网络（HTTP / WebSocket / 超时）。
    Network = 1,
    /// 协议或数据格式（解码、JSON）。
    Protocol = 2,
    /// 鉴权。
    Auth = 3,
    /// 识别会话。
    Asr = 4,
    /// 服务端业务错误。
    Api = 5,
    /// 配置。
    Config = 6,
    /// 输入参数（音频格式、IO）。
    InvalidInput = 7,
    /// 加密。
    Crypto = 8,
}

impl Error {
    /// 错误所属类别。
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Http(_) | Error::WebSocket(_) | Error::Timeout(_) => ErrorKind::Network,
            Error::Decode(_) | Error::Json(_) => ErrorKind::Protocol,
            Error::Auth(_) => ErrorKind::Auth,
            Error::Asr(_) => ErrorKind::Asr,
            Error::Api { .. } => ErrorKind::Api,
            Error::Config(_) => ErrorKind::Config,
            Error::InvalidAudio(_) | Error::Io(_) => ErrorKind::InvalidInput,
            Error::Crypto(_) => ErrorKind::Crypto,
        }
    }

    /// 是否可能通过重试恢复（网络抖动、超时）。
    pub fn is_retryable(&self) -> bool {
        matches!(self.kind(), ErrorKind::Network)
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ErrorKind::Network => "network",
            ErrorKind::Protocol => "protocol",
            ErrorKind::Auth => "auth",
            ErrorKind::Asr => "asr",
            ErrorKind::Api => "api",
            ErrorKind::Config => "config",
            ErrorKind::InvalidInput => "invalid_input",
            ErrorKind::Crypto => "crypto",
        };
        f.write_str(s)
    }
}
